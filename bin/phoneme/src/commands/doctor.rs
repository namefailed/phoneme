use crate::args::DoctorArgs;
use crate::client::Client;
use crate::exit;
use colored::Colorize;
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

    let mut checks = Vec::new();

    // Daemon reachability.
    let mut client_result = Client::connect(cfg).await;
    let daemon_ok = client_result.is_ok();
    checks.push(Check {
        name: "daemon",
        ok: daemon_ok,
        detail: match &client_result {
            Ok(_) => "running".into(),
            Err(_) => "not reachable — run: phoneme daemon start".into(),
        },
    });

    // Daemon status detail (pid) — only if daemon is reachable.
    if let Ok(ref mut c) = client_result {
        match c.send(Request::DaemonStatus).await {
            Ok(value) => checks.push(Check {
                name: "daemon_pid",
                ok: true,
                detail: format!("pid {}", value["pid"]),
            }),
            Err(_) => checks.push(Check {
                name: "daemon_pid",
                ok: false,
                detail: "no status reply".into(),
            }),
        }
    }

    // Filesystem checks. Expand %VAR%/~ so checks reflect real paths.
    let expanded = cfg.expanded();
    let audio_dir_raw = match &expanded {
        Ok(c) => c.recording.audio_dir.clone(),
        Err(_) => cfg.recording.audio_dir.clone(),
    };
    let audio_dir = std::path::Path::new(&audio_dir_raw);
    checks.push(Check {
        name: "audio_dir",
        ok: audio_dir.exists() || std::fs::create_dir_all(audio_dir).is_ok(),
        detail: audio_dir.display().to_string(),
    });

    // Hook executable (best-effort; empty list is treated as ok).
    let hook_cmd = cfg.hook.commands.first().map(String::as_str).unwrap_or("");
    let hook_first_word = hook_cmd.split_whitespace().next().unwrap_or("");
    let (hook_ok, hook_detail) = if hook_first_word.is_empty() {
        (true, "none configured".into())
    } else {
        (
            which::which(hook_first_word).is_ok() || std::path::Path::new(hook_first_word).exists(),
            hook_cmd.to_owned(),
        )
    };
    checks.push(Check {
        name: "hook_executable",
        ok: hook_ok,
        detail: hook_detail,
    });

    // Whisper server reachability.
    let whisper_url = cfg.whisper.server_base_url();
    let whisper_probe = format!("{whisper_url}/health");
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();
    let (whisper_ok, whisper_detail) = match http.get(&whisper_probe).send().await {
        Ok(r) => (
            r.status().is_success() || r.status().as_u16() == 404,
            format!("{whisper_url} — HTTP {}", r.status()),
        ),
        Err(e) if e.is_timeout() => (false, format!("{whisper_url} — timed out")),
        Err(_) => (false, format!("{whisper_url} — not reachable")),
    };
    checks.push(Check {
        name: "whisper_server",
        ok: whisper_ok,
        detail: whisper_detail,
    });

    // Ollama (optional) — probe default port.
    let ollama_required = cfg.llm_post_process.enabled && cfg.llm_post_process.provider == "ollama";
    let (probe_ok, probe_detail) = match http.get("http://127.0.0.1:11434/api/tags").send().await {
        Ok(r) => (
            r.status().is_success(),
            format!("http://127.0.0.1:11434 — HTTP {}", r.status()),
        ),
        Err(e) if e.is_timeout() => (false, "http://127.0.0.1:11434 — timed out".into()),
        Err(_) => (false, "http://127.0.0.1:11434 — not running".into()),
    };
    let (ollama_ok, ollama_detail) = if ollama_required {
        (probe_ok, probe_detail)
    } else if probe_ok {
        (true, format!("{probe_detail} (optional)"))
    } else {
        (
            true,
            format!("{probe_detail} — optional; not required for your config"),
        )
    };
    checks.push(Check {
        name: "ollama (optional)",
        ok: ollama_ok,
        detail: ollama_detail,
    });

    // Print results.
    let any_failed = checks
        .iter()
        .any(|c| !c.ok && c.name != "ollama (optional)");

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

struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
}
