//! Tracing/logging configuration for the daemon.
//!
//! Called once from `main`, right after `AppState` resolves the log dir.
//! The daemon normally runs detached with no console, so the log file is
//! the only window into it (`%LOCALAPPDATA%\phoneme\logs\` — the path the
//! tray's error messages point users at).
//!
//! - Foreground mode (`--foreground`): pretty logs to stderr.
//! - Background (default): JSON lines to `<log_dir>/daemon.log.YYYY-MM-DD`,
//!   rotated DAILY (tracing-appender has no size-based rotation), with old
//!   days pruned down to `daemon.log_max_files` at startup.
//!
//! The level comes from `daemon.log_level` unless `RUST_LOG` overrides it;
//! the returned guard must live for the whole process or buffered lines are
//! lost on exit.

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
        prune_old_logs(log_dir, cfg.daemon.log_max_files as usize);
        let file_appender = tracing_appender::rolling::daily(log_dir, "daemon.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json().with_writer(non_blocking))
            .init();
        Ok(Some(guard))
    }
}

/// Keep only the newest `max_files` daily log files (`daemon.log.YYYY-MM-DD`).
/// The date suffix sorts lexicographically, so name order IS age order.
/// Best-effort: a locked or unreadable file is skipped, never fatal.
fn prune_old_logs(log_dir: &Path, max_files: usize) {
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return;
    };
    let mut logs: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("daemon.log"))
        })
        .collect();
    if logs.len() <= max_files.max(1) {
        return;
    }
    logs.sort();
    let excess = logs.len() - max_files.max(1);
    for path in logs.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}
