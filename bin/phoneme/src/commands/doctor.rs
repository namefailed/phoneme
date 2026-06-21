//! `phoneme doctor` — health checks, with optional repairs.
//!
//! Observe-only on purpose: "is the daemon running?" is doctor's first
//! finding, so it must never auto-spawn one. The daemon-reachability and
//! pid checks are CLI-specific (`DaemonStatus`); everything else — config,
//! audio dir, hook command, model file, whisper/ollama/provider probes —
//! runs in-process via the same `phoneme_core::doctor` checks the GUI Doctor
//! view uses, so both surfaces always agree. Failures print a category badge
//! (`[critical]`/`[warning]`/`[info]`) plus explanation and fix hint; only
//! non-optional, non-info failures make the run exit 1.
//!
//! `--fix` asks the daemon to `RestartWhisper` when a failed check carries
//! the `restart_whisper` fix action, waits for the respawn, and re-probes.
//! `--rebuild-catalog` is the heavy, destructive hammer: it shuts the daemon
//! down (`Shutdown`), waits (bounded) for the pipe to actually vanish — the
//! dying daemon holds the SQLite handles while finalizing — then deletes
//! catalog.db and its WAL sidecars so the next daemon start begins with an
//! empty catalog. Transcripts, tags, notes and titles live only in the DB and
//! are lost; audio files are kept, since the daemon does not reconstruct rows
//! from audio on startup. It refuses to touch the files if the daemon won't
//! exit.
//!
//! `--reimport` is the non-destructive recovery path: it asks the running
//! daemon to scan the audio directory and re-link any `.wav` that has no
//! catalog row (`ReimportFromDisk`), re-creating the row from the file and
//! re-transcribing it. Nothing is ever deleted.

use crate::args::DoctorArgs;
use crate::client::Client;
use crate::exit;
use colored::Colorize;
use phoneme_core::doctor::{self, CheckCategory, CheckResult};
use phoneme_core::Config;
use phoneme_ipc::Request;
use std::process::ExitCode;

/// How long `--rebuild-catalog` waits for the daemon to actually exit after
/// the Shutdown ACK before touching the database files. More generous than
/// `daemon stop`'s wait: a daemon mid-transcription finalizes the in-flight
/// recording on the way out, and rushing it here corrupts the very file we
/// are trying to rebuild.
const REBUILD_STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

pub async fn run(args: DoctorArgs, cfg: &Config, json: bool) -> ExitCode {
    if args.rebuild_catalog {
        // Stop the daemon first to ensure clean catalog deletion. Use the
        // observe-only path — we want to stop a running daemon if one is up,
        // but there is no point spawning a new one just to shut it down.
        let mut client_result = Client::connect_observe(cfg).await;
        if let Ok(ref mut c) = client_result {
            let _ = c.send(phoneme_ipc::Request::Shutdown).await;
            // Shutdown only acknowledges; the daemon then finalizes recordings
            // and reaps children before it actually exits, holding the SQLite
            // handles the whole time. Deleting the DB the moment the ACK arrives
            // races that teardown — the dying daemon can checkpoint the WAL back
            // into a half-deleted file. Wait (bounded) for the pipe to vanish,
            // the same liveness signal `daemon stop` uses, and refuse to touch
            // the files if it never does.
            if !crate::commands::daemon_cmd::wait_for_pipe_death(
                &cfg.daemon.pipe_name,
                REBUILD_STOP_WAIT,
            )
            .await
            {
                eprintln!(
                    "error: the daemon did not exit within {}s — leaving the catalog \
                     untouched. Stop it first (phoneme daemon stop) and re-run.",
                    REBUILD_STOP_WAIT.as_secs()
                );
                return ExitCode::from(exit::GENERIC_FAIL);
            }
        }

        // Delete the catalog database. Resolve the data-local root the same way
        // the daemon does, honoring PHONEME_DATA_LOCAL, so an overridden data
        // directory is the one we touch — otherwise we'd delete the default
        // catalog.db out from under a daemon pointed elsewhere.
        if let Some(data_local) = resolve_data_local_dir() {
            let catalog_path = data_local.join("catalog.db");
            if catalog_path.exists() {
                if let Err(e) = std::fs::remove_file(&catalog_path) {
                    eprintln!("error: failed to delete catalog.db: {e}");
                    return ExitCode::from(exit::GENERIC_FAIL);
                }
                // Take the WAL sidecars with it (best-effort): a leftover
                // catalog.db-wal next to a brand-new database is at best dead
                // weight and at worst a confusing recovery candidate. The ANN
                // index sidecar (catalog.ann, optional feature) is a disposable
                // derived cache keyed to the now-deleted catalog, so it goes too —
                // the daemon rebuilds it from the fresh DB if ANN is enabled.
                for ext in ["db-wal", "db-shm", "ann"] {
                    let sidecar = data_local.join(format!("catalog.{ext}"));
                    let _ = std::fs::remove_file(sidecar);
                }
                println!("deleted catalog.db");
            } else {
                println!("catalog.db does not exist, nothing to rebuild");
            }
        } else {
            eprintln!("error: could not resolve data directory");
            return ExitCode::from(exit::GENERIC_FAIL);
        }

        println!(
            "catalog deleted — start the daemon for a fresh, empty catalog: phoneme daemon start. \
             Your audio is intact; run `phoneme doctor --reimport` to re-link it."
        );
        return ExitCode::SUCCESS;
    }

    if args.reimport {
        // Non-destructive recovery: ask the running daemon to re-link any audio
        // file with no catalog row. Observe-only — there's no point spawning a
        // daemon just to scan; if one isn't up, tell the user.
        let mut client = match Client::connect_observe(cfg).await {
            Ok(c) => c,
            Err(_) => {
                eprintln!("error: daemon not reachable — start it first: phoneme daemon start");
                return ExitCode::from(exit::GENERIC_FAIL);
            }
        };
        match client
            .send(Request::ReimportFromDisk { dry_run: false })
            .await
        {
            Ok(v) => {
                let n = v.get("count").and_then(|c| c.as_u64()).unwrap_or(0);
                println!("re-imported {n} recording(s) from disk");
                return ExitCode::SUCCESS;
            }
            Err(code) => return code,
        }
    }

    let mut checks: Vec<CheckResult> = Vec::new();

    // Daemon reachability (CLI-specific — the GUI doesn't talk to itself over
    // IPC). The remaining checks are shared with the GUI via `phoneme_core::doctor`.
    // Use the observe-only path: whether the daemon is running is the first
    // thing doctor reports, and silently starting one would hide that.
    let mut client_result = Client::connect_observe(cfg).await;
    let daemon_ok = client_result.is_ok();
    checks.push(CheckResult {
        name: "daemon".into(),
        ok: daemon_ok,
        detail: match &client_result {
            Ok(_) => "running".into(),
            Err(_) => "not reachable — run: phoneme daemon start".into(),
        },
        fix_action: None,
        category: if daemon_ok {
            CheckCategory::Info
        } else {
            CheckCategory::Critical
        },
        explanation:
            "The background daemon does all recording and transcription — nothing works without it."
                .into(),
        fix_hint: (!daemon_ok).then(|| "Run: phoneme daemon start".into()),
    });

    // The bundled whisper-servers may have fallen back off their configured
    // ports (another app held them); the backend probes must follow the live
    // ports the daemon reports, or doctor probes a dead port and disagrees
    // with the GUI Doctor. Default (no live ports) until DaemonStatus answers.
    let mut whisper_ports = doctor::EffectiveWhisperPorts::default();

    // Daemon status detail (pid) — only if daemon is reachable.
    if let Ok(ref mut c) = client_result {
        match c.send(Request::DaemonStatus).await {
            Ok(value) => {
                let port = |k: &str| {
                    value
                        .get(k)
                        .and_then(|v| v.as_u64())
                        .and_then(|n| u16::try_from(n).ok())
                };
                whisper_ports = doctor::EffectiveWhisperPorts {
                    main: port("whisper_effective_port"),
                    preview: port("preview_whisper_effective_port"),
                    in_place: port("dictation_whisper_effective_port"),
                };
                checks.push(CheckResult {
                    name: "daemon_pid".into(),
                    ok: true,
                    detail: format!("pid {}", value["pid"]),
                    fix_action: None,
                    category: CheckCategory::Info,
                    explanation: "Reports the daemon process id, useful when debugging.".into(),
                    fix_hint: None,
                });
            }
            Err(_) => checks.push(CheckResult {
                name: "daemon_pid".into(),
                ok: false,
                detail: "no status reply".into(),
                fix_action: None,
                // The daemon accepted the connection but won't answer — treat
                // it like a down daemon.
                category: CheckCategory::Critical,
                explanation: "Reports the daemon process id, useful when debugging.".into(),
                fix_hint: Some("Restart it: phoneme daemon stop && phoneme daemon start".into()),
            }),
        }
    }

    // Shared local-filesystem + backend-reachability checks (config presence,
    // audio dir, hook command, whisper model, whisper/ollama probes).
    checks.extend(doctor::run_local_checks(cfg));
    checks.extend(doctor::run_backend_checks_with_ports(cfg, &whisper_ports).await);

    // Orphaned audio (audio on disk with no catalog row) needs the catalog, so
    // ask the daemon — its dry-run re-import returns the count. Skipped when the
    // daemon isn't reachable (the count is unknowable without it).
    if let Ok(ref mut c) = client_result {
        if let Ok(v) = c.send(Request::ReimportFromDisk { dry_run: true }).await {
            if let Some(count) = v.get("count").and_then(|n| n.as_u64()) {
                checks.push(doctor::orphan_audio_check_result(count as usize));
            }
        }
    }

    // --fix: when a check the daemon can repair failed (the whisper / preview
    // server probes carry fix_action "restart_whisper"), ask the daemon to
    // sweep + respawn the server(s), wait for them to come up, and re-probe.
    if args.fix {
        let fixable_failed = checks
            .iter()
            .any(|c| !c.ok && c.fix_action.as_deref() == Some("restart_whisper"));
        if !fixable_failed {
            if !json {
                println!("--fix: nothing fixable failed (whisper checks are ok).");
            }
        } else if let Ok(ref mut c) = client_result {
            if !json {
                println!("--fix: restarting the bundled whisper-server(s)…");
            }
            match c.send(Request::RestartWhisper).await {
                Ok(_) => {
                    // Give the supervisors a moment to respawn, then re-probe.
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    // The supervisor's port-fallback may have rebound on a
                    // different port; re-query DaemonStatus to get the live
                    // effective ports before probing, so we don't dial the
                    // old (now-dead) port and falsely report the server down.
                    // Reuse the same `c` borrow that is already live in this
                    // `else if` arm — a second borrow of `client_result` would
                    // not compile here.
                    let fresh_ports = match c.send(Request::DaemonStatus).await {
                        Ok(value) => {
                            let port = |k: &str| {
                                value
                                    .get(k)
                                    .and_then(|v| v.as_u64())
                                    .and_then(|n| u16::try_from(n).ok())
                            };
                            doctor::EffectiveWhisperPorts {
                                main: port("whisper_effective_port"),
                                preview: port("preview_whisper_effective_port"),
                                in_place: port("dictation_whisper_effective_port"),
                            }
                        }
                        // If status fails, fall back to the pre-restart ports
                        // rather than probing blind.
                        Err(_) => whisper_ports,
                    };
                    let recheck = doctor::run_backend_checks_with_ports(cfg, &fresh_ports).await;
                    // Replace the stale backend results with the fresh probes.
                    checks.retain(|c| !recheck.iter().any(|r| r.name == c.name));
                    checks.extend(recheck);
                    if !json {
                        println!("--fix: re-probed after restart.");
                    }
                }
                Err(_) => {
                    if !json {
                        eprintln!("--fix: daemon rejected the restart request.");
                    }
                }
            }
        } else if !json {
            eprintln!("--fix: daemon not reachable — start it first: phoneme daemon start");
        }
    }

    // A check whose name is marked "(optional)" never fails the run, and
    // neither does a failing Info-category check (informational by definition).
    let is_optional = |name: &str| name.to_lowercase().contains("(optional)");
    let any_failed = checks
        .iter()
        .any(|c| !c.ok && c.category != CheckCategory::Info && !is_optional(&c.name));

    if json {
        // Additive output: the original name/ok/detail keys stay untouched;
        // category/explanation/fix_hint are new. fix_action stays GUI-only.
        let arr: Vec<_> = checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "ok": c.ok,
                    "detail": c.detail,
                    "category": c.category.label(),
                    "explanation": c.explanation,
                    "fix_hint": c.fix_hint,
                })
            })
            .collect();
        crate::output::print_json(&serde_json::Value::Array(arr));
    } else {
        for c in &checks {
            let mark = if c.ok {
                "✓".green().to_string()
            } else {
                "✗".red().to_string()
            };
            // Passing rows stay one line; failures get a category badge plus
            // indented explanation and fix-hint lines.
            if c.ok {
                println!("{mark} {:<24} {}", c.name, c.detail);
            } else {
                let badge = match c.category {
                    CheckCategory::Critical => "[critical]".red().to_string(),
                    CheckCategory::Warning => "[warning]".yellow().to_string(),
                    CheckCategory::Info => "[info]".dimmed().to_string(),
                };
                println!("{mark} {:<24} {badge} {}", c.name, c.detail);
                if !c.explanation.is_empty() {
                    println!("  {}", c.explanation.dimmed());
                }
                if let Some(hint) = &c.fix_hint {
                    println!("  {} {hint}", "fix:".yellow());
                }
            }
        }
        if !daemon_ok {
            println!("\n  Tip: run `phoneme daemon start` to launch the daemon.");
        }
    }

    if any_failed {
        ExitCode::from(exit::GENERIC_FAIL)
    } else {
        ExitCode::SUCCESS
    }
}

/// Resolve the local app-data root (where catalog.db lives) the same way the
/// daemon and the shared doctor checks do: the `PHONEME_DATA_LOCAL` override
/// wins, otherwise the platform default. Keeps `--rebuild-catalog` pointed at
/// the volume the daemon actually writes to. Shared with `import-backup`, which
/// must open the same catalog.db the daemon owns.
pub(crate) fn resolve_data_local_dir() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("PHONEME_DATA_LOCAL") {
        if !p.is_empty() {
            return Some(std::path::PathBuf::from(p));
        }
    }
    directories::ProjectDirs::from("", "", "phoneme").map(|d| d.data_local_dir().to_path_buf())
}
