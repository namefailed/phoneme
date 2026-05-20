use thiserror::Error;

/// Single error type for `phoneme-core`. Variants map 1:1 to the IPC
/// `ErrorKind` enum, so the daemon can forward errors without translation.
#[derive(Debug, Error)]
pub enum Error {
    #[error("already recording (id={current})")]
    AlreadyRecording { current: String },

    #[error("no active recording")]
    NotRecording,

    #[error("recording {id} not found")]
    NotFound { id: String },

    #[error("invalid config: {0}")]
    InvalidConfig(String),

    #[error("LLM unreachable at {url}: {source}")]
    LlmUnreachable { url: String, source: reqwest::Error },

    #[error("LLM timed out after {secs}s")]
    LlmTimeout { secs: u64 },

    #[error("LLM returned status {status}: {body}")]
    LlmError { status: u16, body: String },

    #[error("hook failed with exit {code}: {stderr_tail}")]
    HookFailed { code: i32, stderr_tail: String },

    #[error("hook timed out after {secs}s")]
    HookTimeout { secs: u64 },

    #[error("daemon not running")]
    DaemonNotRunning,

    #[error("named pipe already in use by pid {pid}")]
    PipeInUse { pid: u32 },

    #[error("daemon is shutting down")]
    ShuttingDown,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),

    #[error(transparent)]
    SqlxMigrate(#[from] sqlx::migrate::MigrateError),

    #[error(transparent)]
    Toml(#[from] toml::de::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_includes_context() {
        let err = Error::NotFound { id: "abc".into() };
        assert_eq!(format!("{err}"), "recording abc not found");
    }

    #[test]
    fn io_error_converts_automatically() {
        let io: std::io::Error =
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope");
        let err: Error = io.into();
        assert!(matches!(err, Error::Io(_)));
    }

    #[test]
    fn hook_failed_carries_exit_code() {
        let err = Error::HookFailed { code: 2, stderr_tail: "boom".into() };
        let s = format!("{err}");
        assert!(s.contains("exit 2"));
        assert!(s.contains("boom"));
    }
}
