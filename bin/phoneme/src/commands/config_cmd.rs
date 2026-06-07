use crate::args::{ConfigAction, ConfigArgs};
use crate::exit;
use phoneme_core::Config;
use std::process::ExitCode;

pub async fn run(args: ConfigArgs, cfg: &Config) -> ExitCode {
    match args.action {
        Some(ConfigAction::Path) => {
            if let Some(p) = phoneme_core::config::default_config_path() {
                println!("{}", p.display());
                ExitCode::SUCCESS
            } else {
                eprintln!("error: could not resolve config path");
                ExitCode::from(exit::GENERIC_FAIL)
            }
        }
        Some(ConfigAction::Reload) => {
            let mut conn = match crate::client::Client::connect(cfg).await {
                Ok(c) => c,
                Err(e) => return e,
            };
            match conn.send(phoneme_ipc::Request::ReloadConfig).await {
                Ok(_) => {
                    println!("daemon reloaded configuration");
                    ExitCode::SUCCESS
                }
                Err(e) => e,
            }
        }
        Some(ConfigAction::Set { key, value }) => match set_value(cfg, &key, &value) {
            Ok(()) => {
                println!("set {key} = {value}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(exit::INVALID_CONFIG)
            }
        },
        None => match toml::to_string_pretty(cfg) {
            Ok(s) => {
                print!("{s}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(exit::GENERIC_FAIL)
            }
        },
    }
}

fn set_value(cfg: &Config, key: &str, value: &str) -> Result<(), String> {
    // Parse the config as a TOML value to handle different types
    let mut toml_value =
        toml::to_string(cfg).map_err(|e| format!("failed to serialize current config: {e}"))?;

    // Parse as TOML document
    let mut doc = toml_value
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| format!("failed to parse config as TOML: {e}"))?;

    // Split the key by dots and navigate the TOML structure
    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() {
        return Err("key cannot be empty".into());
    }

    // Navigate to the parent of the target key
    let mut current = doc.as_table_mut();
    for (i, &part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            // This is the final key - set the value
            // Try to parse the value as various types
            let toml_val = if let Ok(b) = value.parse::<bool>() {
                toml_edit::Value::from(b)
            } else if let Ok(n) = value.parse::<i64>() {
                toml_edit::Value::from(n)
            } else if let Ok(n) = value.parse::<f64>() {
                toml_edit::Value::from(n)
            } else {
                toml_edit::Value::from(value)
            };

            current[part] = toml_edit::Item::Value(toml_val);
        } else {
            // Navigate deeper
            if !current.contains_key(part) {
                return Err(format!("key path '{key}' does not exist in config"));
            }
            current = current[part]
                .as_table_mut()
                .ok_or_else(|| format!("'{part}' is not a table in config"))?;
        }
    }

    // Write the updated config back to file
    let config_path = phoneme_core::config::default_config_path()
        .ok_or_else(|| "could not resolve config path".to_string())?;

    std::fs::write(&config_path, doc.to_string())
        .map_err(|e| format!("failed to write config: {e}"))?;

    Ok(())
}
