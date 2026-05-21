//! Tracing/logging configuration for the daemon.
//!
//! - Foreground mode: pretty logs to stderr.
//! - Background (default): JSON lines to `<log_dir>/daemon.log` with rolling
//!   appender (10 MB × 5 files, configurable).

use phoneme_core::Config;
use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialize tracing for the daemon. Returns a guard that must be held for
/// the lifetime of the process to keep the background writer alive.
pub fn init(cfg: &Config, log_dir: &Path, foreground: bool) -> anyhow::Result<Option<WorkerGuard>> {
    let level = match cfg.daemon.log_level.as_str() {
        "error" => "error",
        "warn" => "warn",
        "info" => "info",
        "debug" => "debug",
        "trace" => "trace",
        _ => "info",
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("phoneme={level},warn")));

    if foreground {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().with_target(true).with_writer(std::io::stderr))
            .init();
        Ok(None)
    } else {
        std::fs::create_dir_all(log_dir)?;
        let file_appender = tracing_appender::rolling::daily(log_dir, "daemon.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json().with_writer(non_blocking))
            .init();
        Ok(Some(guard))
    }
}
