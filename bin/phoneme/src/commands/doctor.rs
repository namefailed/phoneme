use crate::args::DoctorArgs;
use crate::client::Client;
use crate::exit;
use colored::Colorize;
use phoneme_core::doctor::{self, CheckResult};
use phoneme_core::Config;
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: DoctorArgs, cfg: &Config, json: bool) -> ExitCode {
    if args.rebuild_catalog {
        // Stop the daemon first to ensure clean catalog deletion
        let mut client_result = Client::connect(cfg).await;
        if let Ok(ref mut c) = client_result {
            let _ = c.send(phoneme_ipc::Request::Shutdown).await;
        }

        // Delete the catalog database
        let dirs = directories::ProjectDirs::from("", "", "phoneme")
            .ok_or_else(|| anyhow::anyhow!("could not resolve project directories"));
        if let Ok(dirs) = dirs {
            let catalog_path = dirs.data_local_dir().join("catalog.db");
            if catalog_path.exists() {
                if let Err(e) = std::fs::remove_file(&catalog_path) {
                    eprintln!("error: failed to delete catalog.db: {e}");
                    return ExitCode::from(exit::GENERIC_FAIL);
                }
                println!("deleted catalog.db");
            } else {
                println!("catalog.db does not exist, nothing to rebuild");
            }
        } else {
            eprintln!("error: could not resolve data directory");
            return ExitCode::from(exit::GENERIC_FAIL);
        }

        println!("catalog rebuilt; restart the daemon with: phoneme daemon start");
        return ExitCode::SUCCESS;
    }

    let mut checks: Vec<CheckResult> = Vec::new();

    // Daemon reachability (CLI-specific — the GUI doesn't talk to itself over
    // IPC). The remaining checks are shared with the GUI via `phoneme_core::doctor`.
    let mut client_result = Client::connect(cfg).await;
    let daemon_ok = client_result.is_ok();
    checks.push(CheckResult {
        name: "daemon".into(),
        ok: daemon_ok,
        detail: match &client_result {
            Ok(_) => "running".into(),
            Err(_) => "not reachable — run: phoneme daemon start".into(),
        },
        fix_action: None,
    });

    // Daemon status detail (pid) — only if daemon is reachable.
    if let Ok(ref mut c) = client_result {
        match c.send(Request::DaemonStatus).await {
            Ok(value) => checks.push(CheckResult {
                name: "daemon_pid".into(),
                ok: true,
                detail: format!("pid {}", value["pid"]),
                fix_action: None,
            }),
            Err(_) => checks.push(CheckResult {
                name: "daemon_pid".into(),
                ok: false,
                detail: "no status reply".into(),
                fix_action: None,
            }),
        }
    }

    // Shared local-filesystem + backend-reachability checks (config presence,
    // audio dir, hook command, whisper model, whisper/ollama probes).
    checks.extend(doctor::run_local_checks(cfg));
    checks.extend(doctor::run_backend_checks(cfg).await);

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
                    let recheck = doctor::run_backend_checks(cfg).await;
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

    // A check whose name is marked "(optional)" never fails the run.
    let is_optional = |name: &str| name.to_lowercase().contains("(optional)");
    let any_failed = checks.iter().any(|c| !c.ok && !is_optional(&c.name));

    if json {
        let arr: Vec<_> = checks
            .iter()
            .map(|c| serde_json::json!({"name": c.name, "ok": c.ok, "detail": c.detail}))
            .collect();
        crate::output::print_json(&serde_json::Value::Array(arr));
    } else {
        for c in &checks {
            let mark = if c.ok {
                "✓".green().to_string()
            } else {
                "✗".red().to_string()
            };
            println!("{mark} {:<22} {}", c.name, c.detail);
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
