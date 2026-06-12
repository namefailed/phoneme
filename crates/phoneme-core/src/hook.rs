use crate::error::{Error, Result};
use crate::types::HookPayload;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Debug, Clone)]
pub struct HookResult {
    pub exit_code: i32,
    pub stderr_tail: String,
    pub duration_ms: i64,
}

/// Runs the configured hook subprocess with a JSON payload on stdin.
#[derive(Debug, Clone)]
pub struct HookRunner {
    command: String,
    timeout: Duration,
}

impl HookRunner {
    pub fn new(command: String, timeout: Duration) -> Self {
        Self { command, timeout }
    }

    pub async fn run(&self, payload: &HookPayload) -> Result<HookResult> {
        let json = serde_json::to_vec(payload)?;
        let (program, args) = split_command(&self.command);

        let mut cmd = Command::new(program);
        cmd.args(&args)
            .env("PHONEME_ID", payload.id.as_str())
            .env("PHONEME_AUDIO_PATH", &payload.audio_path)
            .env("PHONEME_TRANSCRIPT", &payload.transcript)
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
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&json).await?;
            drop(stdin);
        }

        // Drain stdout/stderr concurrently with the wait. A hook that writes
        // more than the OS pipe buffer (~64 KB) would otherwise deadlock. The
        // `child` handle is deliberately kept out of the drain future (only
        // its pipe handles go in) so it survives for an explicit kill if the
        // timeout fires — `wait_with_output` would consume it and leave no
        // way to terminate a runaway process.
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let mut stderr_buf = Vec::new();
        let wait_and_drain = async {
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
            let (status, _, _) = tokio::join!(child.wait(), drain_out, drain_err);
            status
        };

        let status = match timeout(self.timeout, wait_and_drain).await {
            Ok(r) => r?,
            Err(_) => {
                // Tokio's `Drop` for `Child` does NOT terminate the process on
                // Windows — without an explicit kill every hook timeout leaks
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
/// The result is additionally capped at [`REDACT_MAX_BYTES`] (8 KiB). The cap
/// is applied AFTER masking, so a cut can never expose half a secret. This is
/// best-effort hygiene against accidental echoes, not a parser for every
/// credential format — prefer over-masking to leaking.
pub fn redact_secrets(text: &str) -> String {
    // Compiled per call like the other small regexes in this crate — this runs
    // on user-initiated hook tests, never in a hot path.
    let prefixed = regex::Regex::new(
        r"\b(?:sk-[A-Za-z0-9_-]{8,}|sk_(?:live|test)_[A-Za-z0-9]{8,}|ghp_[A-Za-z0-9]{8,}|gho_[A-Za-z0-9]{8,}|github_pat_[A-Za-z0-9_]{8,}|AKIA[0-9A-Z]{12,})",
    )
    .unwrap();
    // `{8,}` keeps prose like "bearer of bad news" intact; real bearer tokens
    // are far longer.
    let bearer = regex::Regex::new(r"(?i)\bbearer\s+[A-Za-z0-9._~+/-]{8,}=*").unwrap();
    // The value may be bare or quoted; the key name and the `=` are kept so
    // the user can still tell WHICH assignment their script printed.
    let assigned = regex::Regex::new(
        r#"(?i)\b(api[_-]?key|key|token|password|secret)(\s*=\s*)("[^"]*"|'[^']*'|[^\s"']+)"#,
    )
    .unwrap();

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

    /// Ordinary hook chatter passes through byte-for-byte — including words
    /// that merely CONTAIN a sensitive key name (`monkey=`, `max_tokens=`) and
    /// short prose after "bearer".
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

    /// Output is hard-capped at REDACT_MAX_BYTES (plus the truncation marker),
    /// cutting on a char boundary even for multi-byte text.
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

        // A secret near the cut point is masked BEFORE truncation, so the cap
        // can never expose a half-cut token.
        let mut padded = "y".repeat(REDACT_MAX_BYTES - 20);
        padded.push_str(" sk-abcdef1234567890SECRET");
        let out = redact_secrets(&padded);
        assert!(!out.contains("SECRET"), "secret leaked past the cap");
        assert!(!out.contains("sk-abcdef"), "secret leaked past the cap");
    }
}
