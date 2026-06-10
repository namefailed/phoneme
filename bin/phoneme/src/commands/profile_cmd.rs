use crate::args::{ProfileAction, ProfileArgs};
use crate::client;
use crate::exit;
use phoneme_core::profiles;
use phoneme_core::Config;
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: ProfileArgs, cfg: &Config, is_json: bool) -> ExitCode {
    match args.action {
        ProfileAction::List => list(is_json),
        ProfileAction::Save { name } => save(cfg, &name),
        ProfileAction::Use { name } => switch(cfg, &name).await,
    }
}

/// Snapshot the current live config under a named profile.
fn save(cfg: &Config, name: &str) -> ExitCode {
    match profiles::save_profile(name, cfg) {
        Ok(()) => {
            println!("saved profile \"{name}\"");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(exit::GENERIC_FAIL)
        }
    }
}

fn list(is_json: bool) -> ExitCode {
    match profiles::list_profiles() {
        Ok(names) => {
            if is_json {
                match serde_json::to_string_pretty(&names) {
                    Ok(s) => println!("{s}"),
                    Err(e) => {
                        eprintln!("error formatting JSON: {e}");
                        return ExitCode::from(exit::GENERIC_FAIL);
                    }
                }
            } else if names.is_empty() {
                println!("no saved profiles");
            } else {
                for n in names {
                    println!("{n}");
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(exit::GENERIC_FAIL)
        }
    }
}

async fn switch(cfg: &Config, name: &str) -> ExitCode {
    // Load the profile snapshot and write it over the live config.toml.
    let profile = match profiles::load_profile(name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(exit::NOT_FOUND);
        }
    };

    if let Err(e) = write_config(&profile) {
        eprintln!("error: {e}");
        return ExitCode::from(exit::INVALID_CONFIG);
    }

    // Tell the running daemon to adopt the new config. The daemon may not be
    // running; that's fine — it will read config.toml on next start.
    match client::Client::connect(cfg).await {
        Ok(mut conn) => match conn.send(Request::ReloadConfig).await {
            Ok(_) => {
                println!("switched to profile \"{name}\" and reloaded the daemon");
                ExitCode::SUCCESS
            }
            Err(e) => e,
        },
        Err(_) => {
            // config.toml was already updated; just note the daemon was absent.
            println!("switched to profile \"{name}\" (daemon not running; it will pick up the new config on next start)");
            ExitCode::SUCCESS
        }
    }
}

/// Write `config` to the resolved config.toml path atomically (temp + rename).
fn write_config(config: &Config) -> Result<(), String> {
    config.validate().map_err(|e| e.to_string())?;
    let path = phoneme_core::config::default_config_path()
        .ok_or_else(|| "could not resolve config path".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = toml::to_string_pretty(config).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}
