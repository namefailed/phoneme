use crate::error::{Error, Result};
use crate::types::HookPayload;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
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

        let output = match timeout(self.timeout, child.wait_with_output()).await {
            Ok(r) => r?,
            Err(_) => {
                return Err(Error::HookTimeout {
                    secs: self.timeout.as_secs(),
                });
            }
        };

        let duration_ms = started.elapsed().as_millis() as i64;
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr_tail = tail_chars(&stderr, 4096);

        let code = output.status.code().unwrap_or(-1);
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
    let parts: Vec<String> =
        shlex::split(s).unwrap_or_else(|| s.split_whitespace().map(String::from).collect());
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
