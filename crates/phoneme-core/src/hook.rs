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
        tracing::warn!("shlex failed to parse hook command, falling back to whitespace split: {s:?}");
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
}
