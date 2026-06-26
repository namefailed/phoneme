//! Hook execution — running the user's script after a transcription.
//!
//! This module owns [`HookRunner`], which spawns the configured hook command
//! with the recording's [`HookPayload`] as JSON on stdin. The daemon's pipeline
//! calls it once per recording (and on a "re-fire hook" action). The webhook
//! sibling ([`crate::webhook`]) does the HTTP equivalent.
//!
//! The non-obvious parts are all about not hanging or leaking: the stdin write,
//! the stdout drain, and the stderr drain all run concurrently with the wait,
//! inside the timeout, so a chatty hook that ignores a >64 KiB payload can't
//! deadlock the pipe in either direction, and a timed-out child is explicitly
//! killed (Tokio's `Drop` does not terminate the process on Windows).
//! [`redact_secrets`] is a separate concern — it scrubs credential-shaped text
//! out of subprocess output before it crosses the IPC trust boundary back to the
//! GUI (the hook-test feature).

use crate::error::{Error, Result};
use crate::types::HookPayload;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::time::timeout;

/// Outcome of a successful hook run.
#[derive(Debug, Clone)]
pub struct HookResult {
    /// The process exit code (always `0` here — a non-zero exit is reported as
    /// [`Error::HookFailed`] instead).
    pub exit_code: i32,
    /// The tail of the hook's stderr (capped at ~4 KiB), kept even on success so
    /// the UI can show any warnings the script printed.
    pub stderr_tail: String,
    /// Wall-clock duration of the run, in milliseconds.
    pub duration_ms: i64,
}

/// Runs the configured hook subprocess with a JSON payload on stdin.
#[derive(Debug, Clone)]
pub struct HookRunner {
    command: String,
    timeout: Duration,
}

/// Cap on the `PHONEME_TRANSCRIPT` env var, in bytes: 16 KiB. Windows fails
/// `CreateProcess` if a single env var exceeds ~32,767 UTF-16 code units, so a
/// long meeting transcript handed straight to `.env()` would fail the spawn —
/// and the whole hook with it. The full transcript is always on stdin, so this
/// env copy is a convenience that can safely be truncated. The byte cap stays
/// well under the limit even after UTF-16 expansion of multi-byte text.
const MAX_TRANSCRIPT_ENV_BYTES: usize = 16 * 1024;

/// What a truncated `PHONEME_TRANSCRIPT` is suffixed with, so a script reading
/// the env var can tell it was cut and reach for the full text on stdin.
const TRANSCRIPT_ENV_TRUNCATED_MARKER: &str = "… <truncated, full transcript on stdin>";

impl HookRunner {
    /// A runner for `command` (a shell-style command line) bounded by `timeout`.
    pub fn new(command: String, timeout: Duration) -> Self {
        Self { command, timeout }
    }

    /// Run the hook with `payload` serialized to JSON on stdin.
    ///
    /// The payload is also exposed via the `PHONEME_ID`, `PHONEME_AUDIO_PATH`,
    /// and `PHONEME_TRANSCRIPT` environment variables for scripts that prefer
    /// them. `PHONEME_TRANSCRIPT` is capped at `MAX_TRANSCRIPT_ENV_BYTES` with
    /// a truncation marker — Windows rejects a single env var over ~32 KiB and
    /// would fail the spawn outright; the full, untruncated transcript is always
    /// on stdin (the JSON payload). Returns [`Error::HookFailed`] on a non-zero
    /// exit (carrying the stderr tail) and [`Error::HookTimeout`] if the process
    /// runs past the configured timeout (the process is killed first).
    pub async fn run(&self, payload: &HookPayload) -> Result<HookResult> {
        let json = serde_json::to_vec(payload)?;
        let (program, args) = split_command(&self.command);

        let mut cmd = Command::new(program);
        cmd.args(&args)
            .env("PHONEME_ID", payload.id.as_str())
            .env("PHONEME_AUDIO_PATH", &payload.audio_path)
            .env(
                "PHONEME_TRANSCRIPT",
                truncate_transcript_env(&payload.transcript),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            cmd.current_dir(home);
        }

        // On Windows, prevent a console window from flashing up when the hook
        // subprocess is spawned. CREATE_NO_WINDOW = 0x08000000.
        #[cfg(windows)]
        {
            #[allow(unused_imports)]
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }

        let started = Instant::now();
        let mut child = cmd.spawn()?;

        // Feed stdin from inside the timed, concurrently-drained future, not as a
        // standalone `write_all().await` before the drain starts. The payload
        // embeds the full transcript, which can be far larger than the OS pipe
        // buffer (~64 KB); a hook that ignores stdin and instead chats on
        // stdout/stderr then deadlocks both ways — we block writing stdin while
        // the child blocks writing its undrained output — with no timeout to
        // break it, stalling the serial queue worker that awaits this call.
        // Racing the write against the drain under the same `timeout` keeps the
        // pipes flowing and guarantees an escape. Dropped on completion so the
        // child sees EOF on stdin.
        let mut stdin = child.stdin.take();

        // Drain stdout/stderr concurrently with the wait. A hook that writes
        // more than the OS pipe buffer (~64 KB) would otherwise deadlock. The
        // `child` handle is deliberately kept out of the drain future (only
        // its pipe handles go in) so it survives for an explicit kill if the
        // timeout fires; `wait_with_output` would consume it and leave no way to
        // terminate a runaway process.
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let mut stderr_buf = Vec::new();
        let wait_and_drain = async {
            let feed_in = async {
                if let Some(mut si) = stdin.take() {
                    if let Err(e) = si.write_all(&json).await {
                        // A hook that closes stdin early (it doesn't read the
                        // payload) makes the write fail with a broken pipe —
                        // that's fine, the script just chose not to read; let
                        // the run continue and be judged on its exit code.
                        tracing::debug!("failed to write hook stdin: {e}");
                    }
                    // Drop closes the pipe so the child sees EOF on stdin.
                }
            };
            let drain_out = async {
                if let Some(so) = stdout.as_mut() {
                    let mut sink = Vec::new();
                    if let Err(e) = so.read_to_end(&mut sink).await {
                        tracing::error!("failed to read hook stdout: {e}");
                    }
                }
            };
            let drain_err = async {
                if let Some(se) = stderr.as_mut() {
                    if let Err(e) = se.read_to_end(&mut stderr_buf).await {
                        tracing::error!("failed to read hook stderr: {e}");
                    }
                }
            };
            let (status, _, _, _) = tokio::join!(child.wait(), feed_in, drain_out, drain_err);
            status
        };

        let status = match timeout(self.timeout, wait_and_drain).await {
            Ok(r) => r?,
            Err(_) => {
                // Tokio's `Drop` for `Child` does not terminate the process on
                // Windows, so without an explicit kill every hook timeout leaks
                // a `powershell.exe`.
                if let Err(e) = child.start_kill() {
                    tracing::error!("failed to kill runaway hook process: {e}");
                }
                if let Err(e) = child.wait().await {
                    tracing::error!("failed to wait on killed hook process: {e}");
                }
                return Err(Error::HookTimeout {
                    secs: self.timeout.as_secs(),
                });
            }
        };

        let duration_ms = started.elapsed().as_millis() as i64;
        let stderr_text = String::from_utf8_lossy(&stderr_buf);
        let stderr_tail = tail_chars(&stderr_text, 4096);

        let code = status.code().unwrap_or(-1);
        if code == 0 {
            Ok(HookResult {
                exit_code: 0,
                stderr_tail,
                duration_ms,
            })
        } else {
            Err(Error::HookFailed { code, stderr_tail })
        }
    }
}

/// Split a command string into program and arg list using `shlex` (POSIX-style
/// shell tokenization). Handles single quotes, double quotes, backslash escapes,
/// and common Windows path patterns like `"C:\Program Files\App\bin.exe"`.
///
/// Falls back to whitespace splitting if shlex returns `None` (malformed input
/// like an unterminated quote) — better than crashing.
fn split_command(s: &str) -> (String, Vec<String>) {
    let parts: Vec<String> = shlex::split(s).unwrap_or_else(|| {
        tracing::warn!(
            "shlex failed to parse hook command, falling back to whitespace split: {s:?}"
        );
        s.split_whitespace().map(String::from).collect()
    });
    let mut iter = parts.into_iter();
    let program = iter.next().unwrap_or_default();
    let args: Vec<String> = iter.collect();
    (program, args)
}

/// Clamp a transcript for the `PHONEME_TRANSCRIPT` env var. Returns it unchanged
/// when it fits in [`MAX_TRANSCRIPT_ENV_BYTES`]; otherwise cuts the leading bytes
/// at a char boundary and appends [`TRANSCRIPT_ENV_TRUNCATED_MARKER`]. Never
/// truncates the stdin payload — only this convenience copy — so the spawn can't
/// fail on Windows' single-env-var size limit.
fn truncate_transcript_env(transcript: &str) -> String {
    if transcript.len() <= MAX_TRANSCRIPT_ENV_BYTES {
        return transcript.to_string();
    }
    let mut end = MAX_TRANSCRIPT_ENV_BYTES;
    while !transcript.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = transcript[..end].to_string();
    out.push_str(TRANSCRIPT_ENV_TRUNCATED_MARKER);
    out
}

fn tail_chars(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        s.to_string()
    } else {
        // Take the trailing max_bytes, snap to char boundary
        let start = s.len() - max_bytes;
        let start = s
            .char_indices()
            .find(|(i, _)| *i >= start)
            .map(|(i, _)| i)
            .unwrap_or(start);
        s[start..].to_string()
    }
}

/// Hard cap on text returned from [`redact_secrets`], in bytes: 8 KiB. Longer
/// output is cut at a char boundary and suffixed with a truncation marker.
pub const REDACT_MAX_BYTES: usize = 8 * 1024;

/// What a masked secret is replaced with. Matches the `<redacted>` convention
/// the config `Debug` impls use for API keys.
const REDACTED: &str = "<redacted>";

/// Mask credential-shaped substrings in subprocess output before it crosses a
/// trust boundary (daemon → tray/CLI over IPC). A hook-test command echoes
/// whatever the user's script prints — and a debugging script often dumps its
/// environment or a config file, which is exactly where keys live.
///
/// Masked shapes:
/// - bare tokens with well-known prefixes: `sk-…` (OpenAI/Anthropic-style),
///   `sk_live_…`/`sk_test_…` (Stripe), `ghp_…`/`gho_…`/`github_pat_…`
///   (GitHub), `AKIA…` (AWS access key ids)
/// - `Bearer <token>` authorization values
/// - `key=`/`api_key=`/`token=`/`password=`/`secret=` assignments — the key
///   name survives, the value is masked
///
/// The result is additionally capped at [`REDACT_MAX_BYTES`] (8 KiB). The cap is
/// applied after masking, so a cut can never expose half a secret. This is
/// best-effort hygiene against accidental echoes, not a parser for every
/// credential format — when in doubt it over-masks rather than leak.
pub fn redact_secrets(text: &str) -> String {
    // Compiled per call like the other small regexes in this crate — this runs
    // on user-initiated hook tests, never in a hot path.
    let prefixed = regex::Regex::new(
        r"\b(?:sk-[A-Za-z0-9_-]{8,}|sk_(?:live|test)_[A-Za-z0-9]{8,}|ghp_[A-Za-z0-9]{8,}|gho_[A-Za-z0-9]{8,}|github_pat_[A-Za-z0-9_]{8,}|AKIA[0-9A-Z]{12,})",
    )
    .expect("valid static regex");
    // `{8,}` keeps prose like "bearer of bad news" intact; real bearer tokens
    // are far longer.
    let bearer =
        regex::Regex::new(r"(?i)\bbearer\s+[A-Za-z0-9._~+/-]{8,}=*").expect("valid static regex");
    // The value may be bare or quoted; the key name and the `=` are kept so the
    // user can still tell which assignment their script printed.
    let assigned = regex::Regex::new(
        r#"(?i)\b(api[_-]?key|key|token|password|secret)(\s*=\s*)("[^"]*"|'[^']*'|[^\s"']+)"#,
    )
    .expect("valid static regex");

    let masked = prefixed.replace_all(text, REDACTED);
    let masked = bearer.replace_all(&masked, format!("Bearer {REDACTED}"));
    let mut masked = assigned
        .replace_all(&masked, format!("${{1}}${{2}}{REDACTED}"))
        .into_owned();

    if masked.len() > REDACT_MAX_BYTES {
        let mut end = REDACT_MAX_BYTES;
        while !masked.is_char_boundary(end) {
            end -= 1;
        }
        masked.truncate(end);
        masked.push_str("… <truncated>");
    }
    masked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_command_handles_unquoted() {
        let (p, a) = split_command("powershell -File foo.ps1");
        assert_eq!(p, "powershell");
        assert_eq!(a, vec!["-File", "foo.ps1"]);
    }

    #[test]
    fn split_command_handles_quoted_paths() {
        let (p, a) = split_command("powershell -File \"C:/Program Files/x.ps1\"");
        assert_eq!(p, "powershell");
        assert_eq!(a, vec!["-File", "C:/Program Files/x.ps1"]);
    }

    /// A hook that never reads stdin while flooding stdout, handed a payload far
    /// larger than the OS pipe buffer, must still finish under the timeout: the
    /// stdin write races the drain rather than blocking ahead of it. The failure
    /// mode this guards against is a deadlock — writing the >64 KiB transcript
    /// blocks while the child blocks writing its undrained stdout, with no
    /// timeout escape, and the serial queue worker awaiting the call stalls too.
    #[tokio::test]
    async fn run_does_not_deadlock_on_large_payload_with_chatty_hook() {
        // A shell one-liner that ignores stdin entirely and prints ~200 KiB to
        // stdout — comfortably past the ~64 KiB pipe buffer in both directions.
        #[cfg(windows)]
        let command = "cmd /c \"for /L %i in (1,1,4000) do @echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"".to_string();
        #[cfg(not(windows))]
        let command =
            "sh -c 'i=0; while [ $i -lt 4000 ]; do echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa; i=$((i+1)); done'"
                .to_string();

        let runner = HookRunner::new(command, Duration::from_secs(30));
        let mut payload = sample_payload();
        // ~256 KiB transcript: bigger than the stdin pipe buffer, so a naive
        // pre-drain `write_all` would block before the child's output is read.
        payload.transcript = "x".repeat(256 * 1024);

        let started = Instant::now();
        let result = runner.run(&payload).await;
        // The hook exits 0, so this is Ok — the assertion that matters is that it
        // returned well under the timeout instead of hanging.
        assert!(result.is_ok(), "chatty hook should succeed: {result:?}");
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "hook deadlocked instead of streaming the pipes: {:?}",
            started.elapsed()
        );
    }

    /// A short transcript passes through `PHONEME_TRANSCRIPT` byte-for-byte; a
    /// long one is cut at the cap and marked, so it can never blow Windows'
    /// single-env-var size limit and fail the spawn.
    #[test]
    fn truncate_transcript_env_caps_long_transcripts() {
        let short = "hello world";
        assert_eq!(truncate_transcript_env(short), short);

        let long = "x".repeat(MAX_TRANSCRIPT_ENV_BYTES * 4);
        let out = truncate_transcript_env(&long);
        assert!(
            out.ends_with(TRANSCRIPT_ENV_TRUNCATED_MARKER),
            "marker: {out}"
        );
        assert!(
            out.len() <= MAX_TRANSCRIPT_ENV_BYTES + TRANSCRIPT_ENV_TRUNCATED_MARKER.len(),
            "cap not enforced: {} bytes",
            out.len()
        );

        // Multi-byte text must cut on a char boundary, never panic.
        let unicode = "€".repeat(MAX_TRANSCRIPT_ENV_BYTES); // 3 bytes each
        let out = truncate_transcript_env(&unicode);
        assert!(out.ends_with(TRANSCRIPT_ENV_TRUNCATED_MARKER));
    }

    /// A multi-hundred-KiB transcript must not fail the spawn. The same text is
    /// also set as the `PHONEME_TRANSCRIPT` env var; on Windows an untruncated
    /// copy would exceed the ~32 KiB single-var limit and make `CreateProcess`
    /// fail, taking the whole hook down with an `Io` error. The truncated env
    /// copy keeps the spawn alive; the full transcript still rides on stdin.
    #[tokio::test]
    async fn run_spawns_with_huge_transcript() {
        #[cfg(windows)]
        let command = "cmd /c exit 0".to_string();
        #[cfg(not(windows))]
        let command = "sh -c 'cat >/dev/null; exit 0'".to_string();

        let runner = HookRunner::new(command, Duration::from_secs(30));
        let mut payload = sample_payload();
        payload.transcript = "x".repeat(512 * 1024); // 512 KiB, far past the env limit

        let result = runner.run(&payload).await;
        assert!(
            result.is_ok(),
            "spawn must survive a huge transcript: {result:?}"
        );
    }

    /// Build a minimal payload for the subprocess tests.
    fn sample_payload() -> HookPayload {
        HookPayload {
            id: crate::id::RecordingId::new(),
            timestamp: chrono::Local::now(),
            transcript: String::new(),
            audio_path: "test.wav".into(),
            duration_ms: 1000,
            model: "test".into(),
            metadata: crate::types::HookMetadata::current(),
        }
    }

    #[test]
    fn tail_chars_short_string_unchanged() {
        assert_eq!(tail_chars("hello", 100), "hello");
    }

    #[test]
    fn tail_chars_trims_long_string() {
        let s = "x".repeat(10_000);
        let t = tail_chars(&s, 100);
        assert_eq!(t.len(), 100);
    }

    // ── redact_secrets ──────────────────────────────────────────────────────

    /// Every well-known token prefix is masked, and the original secret never
    /// survives into the output.
    #[test]
    fn redact_masks_prefixed_tokens() {
        // Each fixture is split with concat! so secret scanners (e.g. GitHub
        // push protection) never see a contiguous token in the source; the
        // runtime string the redactor scans is identical either way.
        let secrets = [
            concat!("sk-proj-", "abc123DEF456ghi789"),
            concat!("sk_live_", "4eC39HqLyjWDarjtT1zdp7dc"),
            concat!("sk_test_", "4eC39HqLyjWDarjtT1zdp7dc"),
            concat!("ghp_", "16C7e42F292c6912E7710c838347Ae178B4a"),
            concat!("gho_", "16C7e42F292c6912E7710c838347Ae178B4a"),
            concat!("github_pat_", "11ABCDEFG0_abcdefghijklmnop"),
            concat!("AKIA", "IOSFODNN7EXAMPLE"),
        ];
        for secret in secrets {
            let input = format!("debug: found {secret} in env");
            let out = redact_secrets(&input);
            assert!(!out.contains(secret), "{secret} must not survive: {out}");
            assert!(out.contains("<redacted>"), "mask expected in: {out}");
            assert!(out.contains("debug: found"), "context survives: {out}");
        }
    }

    /// `Bearer <token>` values are masked regardless of header casing.
    #[test]
    fn redact_masks_bearer_tokens() {
        let out = redact_secrets("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig");
        assert_eq!(out, "Authorization: Bearer <redacted>");
        let out = redact_secrets("authorization: bearer abc123def456");
        assert!(!out.contains("abc123def456"), "got: {out}");
        assert!(out.contains("Bearer <redacted>"), "got: {out}");
    }

    /// `key=`-style assignments keep the key name (so the user can tell which
    /// assignment their script printed) but lose the value — bare or quoted,
    /// any casing, with or without spaces around the `=`.
    #[test]
    fn redact_masks_assigned_values_keeping_key_names() {
        let cases = [
            ("key=hunter2value", "key=<redacted>"),
            ("api_key=abcd1234", "api_key=<redacted>"),
            ("API-KEY=abcd1234", "API-KEY=<redacted>"),
            ("token = xyz987", "token = <redacted>"),
            ("password=\"correct horse\"", "password=<redacted>"),
            ("secret='battery staple'", "secret=<redacted>"),
        ];
        for (input, want) in cases {
            assert_eq!(redact_secrets(input), want);
        }
    }

    /// Ordinary hook chatter passes through byte-for-byte, including words that
    /// merely contain a sensitive key name (`monkey=`, `max_tokens=`) and short
    /// prose after "bearer".
    #[test]
    fn redact_leaves_benign_text_untouched() {
        let benign = [
            "hook finished in 320ms, wrote 2 files",
            "monkey=banana and donkey=carrot",
            "max_tokens=256 temperature=0.7",
            "the bearer of bad news",
            "ask Skylar about the demo", // "sk" without the token shape
        ];
        for input in benign {
            assert_eq!(redact_secrets(input), input, "benign text must survive");
        }
    }

    /// Output is hard-capped at [`REDACT_MAX_BYTES`] (plus the truncation
    /// marker), cutting on a char boundary even for multi-byte text.
    #[test]
    fn redact_caps_output_length() {
        let huge = "x".repeat(REDACT_MAX_BYTES * 3);
        let out = redact_secrets(&huge);
        assert!(
            out.len() <= REDACT_MAX_BYTES + "… <truncated>".len(),
            "cap not enforced: {} bytes",
            out.len()
        );
        assert!(out.ends_with("<truncated>"), "marker expected");

        // Multi-byte chars: must not panic and must stay within the cap.
        let unicode = "€".repeat(REDACT_MAX_BYTES); // 3 bytes each
        let out = redact_secrets(&unicode);
        assert!(out.len() <= REDACT_MAX_BYTES + "… <truncated>".len());

        // A secret near the cut point is masked before truncation, so the cap
        // can never expose a half-cut token.
        let mut padded = "y".repeat(REDACT_MAX_BYTES - 20);
        padded.push_str(" sk-abcdef1234567890SECRET");
        let out = redact_secrets(&padded);
        assert!(!out.contains("SECRET"), "secret leaked past the cap");
        assert!(!out.contains("sk-abcdef"), "secret leaked past the cap");
    }
}
