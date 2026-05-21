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

fn set_value(_cfg: &Config, key: &str, _value: &str) -> Result<(), String> {
    // For v1 the wizard handles config editing; CLI `set` is a stretch goal.
    Err(format!(
        "setting `{key}` via CLI is not yet implemented; edit config.toml directly"
    ))
}
