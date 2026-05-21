use crate::args::DoctorArgs;
use crate::client::Client;
use crate::exit;
use colored::Colorize;
use phoneme_core::Config;
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: DoctorArgs, cfg: &Config, json: bool) -> ExitCode {
    if args.rebuild_catalog {
        eprintln!(
            "doctor --rebuild-catalog is not yet implemented as a CLI; run the daemon's catalog rebuild command"
        );
        return ExitCode::from(exit::GENERIC_FAIL);
    }

    let mut checks = Vec::new();

    // Daemon reachability.
    let mut client_result = Client::connect(cfg).await;
    checks.push(Check {
        name: "daemon",
        ok: client_result.is_ok(),
        detail: match &client_result {
            Ok(_) => "running".into(),
            Err(_) => "not reachable".into(),
        },
    });

    // Daemon status detail.
    if let Ok(ref mut c) = client_result {
        match c.send(Request::DaemonStatus).await {
            Ok(value) => {
                checks.push(Check {
                    name: "daemon_status",
                    ok: true,
                    detail: format!("pid {}", value["pid"]),
                });
            }
            Err(_) => checks.push(Check {
                name: "daemon_status",
                ok: false,
                detail: "no status reply".into(),
            }),
        }
    }

    // Filesystem checks.
    let audio_dir = std::path::Path::new(&cfg.recording.audio_dir);
    checks.push(Check {
        name: "audio_dir",
        ok: audio_dir.exists() || std::fs::create_dir_all(audio_dir).is_ok(),
        detail: audio_dir.display().to_string(),
    });

    // Hook file (best-effort).
    let hook_first_word = cfg.hook.command.split_whitespace().next().unwrap_or("");
    checks.push(Check {
        name: "hook_executable",
        ok: which::which(hook_first_word).is_ok() || std::path::Path::new(hook_first_word).exists(),
        detail: hook_first_word.into(),
    });

    let any_failed = checks.iter().any(|c| !c.ok);

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
            println!("{mark} {:<20} {}", c.name, c.detail);
        }
    }

    if any_failed {
        ExitCode::from(exit::GENERIC_FAIL)
    } else {
        ExitCode::SUCCESS
    }
}

struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
}
