//! `phoneme config` — inspect and edit the shared config.toml.
//!
//! With no subcommand, prints the full resolved config as TOML. `path`
//! prints the config file location; both are purely local. `reload` sends
//! `ReloadConfig` (spawning path) so a daemon re-reads the file.
//!
//! `set <key> <value>` edits the file directly — no daemon involved (run
//! `config reload` after, or let the queue worker's mtime check pick it
//! up). Dotted keys navigate tables (`whisper.mode`), values are
//! type-sniffed (bool → int → float → string), and the FULL updated config
//! is parsed and validated BEFORE anything touches disk — a bad value must
//! never brick the config file. Writes go atomically (tmp + rename) to the
//! SAME file the daemon reads (`PHONEME_CONFIG`-aware). Exits 6 on any
//! rejected value.

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
    let toml_value =
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

    // Validate the FULL updated config before anything touches the disk: a
    // value that doesn't parse back (wrong type for the field) or that fails
    // `Config::validate()` would otherwise be written out and then rejected by
    // the daemon on its next load — bricking every later `phoneme` invocation
    // until the file is hand-repaired.
    let updated: Config = toml::from_str(&doc.to_string())
        .map_err(|e| format!("'{value}' is not valid for {key}: {e}"))?;
    updated
        .validate()
        .map_err(|e| format!("rejected: the change makes the config invalid: {e}"))?;

    // Write to the SAME file the daemon reads: the PHONEME_CONFIG override
    // when set, else the per-user default. Writing default_config_path
    // unconditionally (the old behavior) silently edited a file the daemon
    // never looks at whenever the override was active.
    let config_path = phoneme_core::config::resolved_config_path()
        .ok_or_else(|| "could not resolve config path".to_string())?;
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create config directory: {e}"))?;
    }

    // Atomic replace: write a sibling tmp file, then rename over the target.
    // A crash mid-write leaves the old config intact instead of a truncated
    // half-file the daemon can no longer parse.
    let tmp_path = config_path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, doc.to_string())
        .map_err(|e| format!("failed to write config: {e}"))?;
    if let Err(e) = std::fs::rename(&tmp_path, &config_path) {
        // Windows rename can fail if the target is momentarily locked; don't
        // leave the tmp file behind in that case.
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("failed to replace config file: {e}"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point `PHONEME_CONFIG` at a temp file for the duration of one test,
    /// restoring the previous value on drop so tests can't leak into each
    /// other (the suite runs with --test-threads=1).
    struct ConfigEnvOverride {
        prev: Option<String>,
    }

    impl ConfigEnvOverride {
        fn set(path: &std::path::Path) -> Self {
            let prev = std::env::var("PHONEME_CONFIG").ok();
            std::env::set_var("PHONEME_CONFIG", path);
            Self { prev }
        }
    }

    impl Drop for ConfigEnvOverride {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => std::env::set_var("PHONEME_CONFIG", v),
                None => std::env::remove_var("PHONEME_CONFIG"),
            }
        }
    }

    #[test]
    fn set_value_writes_to_the_phoneme_config_override_path() {
        // Regression: `config set` wrote default_config_path unconditionally,
        // so with PHONEME_CONFIG active it edited a file the daemon never reads.
        let tmp = tempfile::tempdir().unwrap();
        let override_path = tmp.path().join("override.toml");
        let _env = ConfigEnvOverride::set(&override_path);

        let cfg = Config::default();
        set_value(&cfg, "recording.sample_rate", "32000").expect("set succeeds");

        let written =
            std::fs::read_to_string(&override_path).expect("the override file is the one written");
        assert!(
            written.contains("sample_rate = 32000"),
            "the new value must land in the override file: {written}"
        );
        // And the result is a loadable config.
        let reloaded: Config = toml::from_str(&written).unwrap();
        assert_eq!(reloaded.recording.sample_rate, 32000);
    }

    #[test]
    fn set_value_rejects_values_that_fail_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let override_path = tmp.path().join("override.toml");
        let _env = ConfigEnvOverride::set(&override_path);

        let cfg = Config::default();
        // validate() bounds sample_rate to 8000..=96000.
        let err = set_value(&cfg, "recording.sample_rate", "4000")
            .expect_err("an out-of-range value must be rejected");
        assert!(
            err.contains("invalid"),
            "the error should say the config became invalid: {err}"
        );
        assert!(
            !override_path.exists(),
            "nothing may be written when validation fails"
        );
    }

    #[test]
    fn set_value_rejects_type_mismatches_before_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let override_path = tmp.path().join("override.toml");
        let _env = ConfigEnvOverride::set(&override_path);

        let cfg = Config::default();
        // A string where an integer field lives fails the parse-back check.
        let err = set_value(&cfg, "recording.sample_rate", "very-fast")
            .expect_err("a type mismatch must be rejected");
        assert!(err.contains("not valid for recording.sample_rate"), "{err}");
        assert!(!override_path.exists());
    }

    #[test]
    fn set_value_leaves_no_tmp_file_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let override_path = tmp.path().join("override.toml");
        let _env = ConfigEnvOverride::set(&override_path);

        let cfg = Config::default();
        set_value(&cfg, "daemon.log_level", "debug").expect("set succeeds");

        assert!(override_path.exists(), "the real file exists");
        assert!(
            !override_path.with_extension("toml.tmp").exists(),
            "the tmp sibling must be renamed away, not left behind"
        );
        let written = std::fs::read_to_string(&override_path).unwrap();
        assert!(written.contains("log_level = \"debug\""));
    }
}
