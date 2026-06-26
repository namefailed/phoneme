//! `phoneme import-backup <ZIP>` — restore a library backup (inverse of export).
//!
//! `phoneme export <FILE>` writes a zip of the whole library — a `catalog.json`
//! envelope plus every `.wav`. This restores one back: the recordings are
//! re-inserted into the catalog and their audio copied into the configured audio
//! dir. Restore is **idempotent** — a recording whose id already exists is
//! skipped (counted, never overwritten), so re-running on the same backup is
//! safe and a hand edit made since survives.
//!
//! Unlike most subcommands, this does local work against the catalog database
//! directly (like `doctor --rebuild-catalog` and `config set`): the daemon owns
//! catalog.db while it runs, so this first asks a running daemon to shut down and
//! waits — bounded — for it to release the file before opening it. The core
//! restore logic lives in `phoneme_core::backup`; this is the thin wrapper that
//! resolves paths, frees the daemon, and reports counts.

use crate::args::ImportBackupArgs;
use crate::client::Client;
use crate::commands::daemon_cmd::wait_for_pipe_death;
use crate::commands::doctor::resolve_data_local_dir;
use crate::exit;
use phoneme_core::{backup, Catalog, Config};
use phoneme_ipc::NamedPipeTransport;
use std::path::Path;
use std::process::ExitCode;

/// How long to wait for the daemon to actually exit (release catalog.db) after
/// it ACKs the shutdown. Matches `doctor --rebuild-catalog`: a daemon
/// mid-transcription finalizes the in-flight recording on the way out, and
/// touching the DB before it lets go risks corrupting it.
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

pub async fn run(args: ImportBackupArgs, cfg: &Config) -> ExitCode {
    let zip_path = Path::new(&args.file);
    if !zip_path.exists() {
        eprintln!("error: backup file not found: {}", zip_path.display());
        return ExitCode::from(exit::NOT_FOUND);
    }

    // The daemon holds catalog.db open. Stop a running one first (observe-only —
    // no point spawning a daemon just to shut it down) and wait for it to
    // release the file, refusing to touch the DB if it won't exit.
    match Client::connect_observe(cfg).await {
        Ok(mut c) => {
            let _ = c.send(phoneme_ipc::Request::Shutdown).await;
            if !wait_for_pipe_death(&cfg.daemon.pipe_name, STOP_WAIT).await {
                eprintln!(
                    "error: the daemon did not exit within {}s — leaving the catalog \
                     untouched. Stop it first (phoneme daemon stop) and re-run.",
                    STOP_WAIT.as_secs()
                );
                return ExitCode::from(exit::GENERIC_FAIL);
            }
        }
        Err(_) => {
            // connect_observe failed for one of two reasons: no daemon (safe —
            // nothing holds the DB) or a live but protocol-incompatible daemon
            // the handshake rejected (it's still holding catalog.db). Tell them
            // apart with a plain pipe probe: if the pipe still answers, a daemon
            // is alive — refuse to open the DB rather than risk corrupting it.
            if NamedPipeTransport::connect(&cfg.daemon.pipe_name)
                .await
                .is_ok()
            {
                eprintln!(
                    "error: could not confirm the daemon is stopped (it's running but \
                     speaks an incompatible protocol). Stop it first (phoneme daemon \
                     stop) and re-run — leaving the catalog untouched."
                );
                return ExitCode::from(exit::GENERIC_FAIL);
            }
            // Pipe is dead — no daemon, safe to proceed.
        }
    }

    // Resolve the catalog + audio paths the same way the daemon does, so we
    // restore into the library the daemon will read on its next start.
    let data_local = match resolve_data_local_dir() {
        Some(d) => d,
        None => {
            eprintln!("error: could not resolve data directory");
            return ExitCode::from(exit::GENERIC_FAIL);
        }
    };
    let catalog_path = data_local.join("catalog.db");

    let expanded = match cfg.expanded() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return ExitCode::from(exit::INVALID_CONFIG);
        }
    };
    let audio_dir = std::path::PathBuf::from(&expanded.recording.audio_dir);

    let catalog = match Catalog::open(&catalog_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "error: could not open catalog at {}: {e}",
                catalog_path.display()
            );
            return ExitCode::from(exit::GENERIC_FAIL);
        }
    };

    match backup::restore_from_zip(zip_path, &catalog, &audio_dir).await {
        Ok(report) => {
            println!(
                "restored {} recording(s), skipped {} already present",
                report.imported, report.skipped
            );
            println!("start the daemon to use them: phoneme daemon start");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: restore failed: {e}");
            ExitCode::from(exit::GENERIC_FAIL)
        }
    }
}
