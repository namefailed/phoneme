//! The crate's single error type.
//!
//! Every fallible function in `phoneme-core` returns [`Result`], which is
//! [`enum@Error`] in its `Err` slot. The daemon forwards these straight over IPC: the
//! variants map 1:1 to the wire `ErrorKind`, so a failure surfaces to the CLI or
//! GUI with the same identity it had here, no translation layer in between.
//!
//! Two flavours of variant: the *domain* ones (`AlreadyRecording`, `NotFound`,
//! the `Whisper*`/`Hook*` failures, …) carry the context a caller wants to act
//! on, and the *transparent* ones (`Io`, `Sqlx`, `Toml`, `Json`, …) wrap a lower
//! library's error via `#[from]` so the `?` operator just works. `Internal` is
//! the catch-all for an invariant that should never break in practice.

use thiserror::Error;

/// Single error type for `phoneme-core`. Variants map 1:1 to the IPC
/// `ErrorKind` enum, so the daemon can forward errors without translation.
#[derive(Debug, Error)]
pub enum Error {
    /// A recording was requested but one is already in progress.
    #[error("already recording (id={current})")]
    AlreadyRecording {
        /// The id of the recording that already holds the slot.
        current: String,
    },

    /// A stop/cancel/finalize was requested but nothing is recording.
    #[error("no active recording")]
    NotRecording,

    /// No recording exists with the given id.
    #[error("recording {id} not found")]
    NotFound {
        /// The id that did not resolve (or a label like `profile "work"`).
        id: String,
    },

    /// A config value failed validation (the message names the bad field).
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    /// The transcription endpoint could not be reached (DNS/TCP/TLS failure).
    #[error("Whisper unreachable at {url}: {source}")]
    WhisperUnreachable {
        /// The endpoint URL that was being contacted.
        url: String,
        /// The underlying transport error.
        source: reqwest::Error,
    },

    /// Transcription did not complete within the configured timeout.
    #[error("Whisper timed out after {secs}s")]
    WhisperTimeout {
        /// The timeout that elapsed, in seconds.
        secs: u64,
    },

    /// The transcription endpoint answered with a non-success HTTP status.
    #[error("Whisper returned status {status}: {body}")]
    WhisperError {
        /// The HTTP status code returned.
        status: u16,
        /// The (length-capped) response body, for diagnosis.
        body: String,
    },

    /// A hook subprocess exited non-zero.
    #[error("hook failed with exit {code}: {stderr_tail}")]
    HookFailed {
        /// The process exit code (`-1` when it couldn't be determined).
        code: i32,
        /// The tail of the hook's stderr (capped), for diagnosis.
        stderr_tail: String,
    },

    /// A hook subprocess (or a webhook POST) ran past its timeout and was killed.
    #[error("hook timed out after {secs}s")]
    HookTimeout {
        /// The timeout that elapsed, in seconds.
        secs: u64,
    },

    /// The CLI/tray tried to talk to a daemon that isn't running.
    #[error("daemon not running")]
    DaemonNotRunning,

    /// The IPC named pipe is already held by another daemon process.
    #[error("named pipe already in use by pid {pid}")]
    PipeInUse {
        /// The process id currently holding the pipe.
        pid: u32,
    },

    /// The daemon is shutting down and is refusing new work.
    #[error("daemon is shutting down")]
    ShuttingDown,

    /// A filesystem or other I/O operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A SQLite query failed.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),

    /// A database migration failed to apply.
    #[error(transparent)]
    SqlxMigrate(#[from] sqlx::migrate::MigrateError),

    /// A `config.toml` failed to parse.
    #[error(transparent)]
    Toml(#[from] toml::de::Error),

    /// JSON (de)serialization failed (payloads, suggestions, tag lists).
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// An internal invariant broke — a bug, a poisoned lock, or an
    /// otherwise-unexpected state. The message carries the detail.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Crate-wide result alias: `Result<T>` is `std::result::Result<T, Error>`.
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
        let io: std::io::Error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope");
        let err: Error = io.into();
        assert!(matches!(err, Error::Io(_)));
    }

    #[test]
    fn hook_failed_carries_exit_code() {
        let err = Error::HookFailed {
            code: 2,
            stderr_tail: "boom".into(),
        };
        let s = format!("{err}");
        assert!(s.contains("exit 2"));
        assert!(s.contains("boom"));
    }
}
