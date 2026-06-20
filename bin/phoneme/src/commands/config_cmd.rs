//! `phoneme config` — inspect and edit the shared config.toml.
//!
//! With no subcommand, prints the full resolved config as TOML with secret
//! values (API keys, the webhook HMAC secret) masked, so the dump is safe to
//! paste or pipe even off Windows where secrets aren't DPAPI-encrypted at rest.
//! Pass `--show-secrets` to print the real values when you deliberately need
//! them.
//! `path` prints the config file location; both are purely local. `reload`
//! sends `ReloadConfig` (spawning path) so a daemon re-reads the file.
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
        None => match render_config(cfg, args.show_secrets) {
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

/// The placeholder shown in `phoneme config` for a non-empty secret value.
const REDACTED_SECRET: &str = "<redacted>";

/// Serialize `cfg` to TOML for display, masking every secret value unless
/// `show_secrets` is set.
///
/// `Config` serializes secrets through the DPAPI protector — on Windows they
/// already land as opaque `dpapi:v1:…` ciphertext, but off Windows `protect()`
/// is a passthrough, so a plain `phoneme config` would print API keys and the
/// webhook HMAC secret in cleartext to the terminal (and into any shell history
/// or piped log). Mask them in the rendered TOML regardless of platform so the
/// dump is safe to share by default; `--show-secrets` opts back into the real
/// values for the rare case the user deliberately needs them. The on-disk file
/// is untouched; `config set` still writes real values. The same field set as
/// the GUI's `mask_config_secrets`.
fn render_config(cfg: &Config, show_secrets: bool) -> Result<String, String> {
    let serialized =
        toml::to_string_pretty(cfg).map_err(|e| format!("failed to serialize config: {e}"))?;
    let mut doc = serialized
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| format!("failed to parse config for display: {e}"))?;
    if !show_secrets {
        redact_secrets_for_display(&mut doc);
    }
    Ok(doc.to_string())
}

/// Mask every secret value in `doc` in place with [`REDACTED_SECRET`], leaving
/// empty values alone (an unset key is not a secret and the blank reads clearer).
fn redact_secrets_for_display(doc: &mut toml_edit::DocumentMut) {
    // Top-level provider tables that each carry an `api_key`.
    for section in [
        "whisper",
        "preview_whisper",
        "llm_post_process",
        "summary",
        "auto_tag",
        "title",
    ] {
        if let Some(table) = doc.get_mut(section).and_then(|s| s.as_table_like_mut()) {
            mask_field(table, "api_key");
        }
    }
    // The dictation STT key lives one level deeper (`in_place.stt.api_key`).
    if let Some(stt) = doc
        .get_mut("in_place")
        .and_then(|s| s.as_table_like_mut())
        .and_then(|t| t.get_mut("stt"))
        .and_then(|s| s.as_table_like_mut())
    {
        mask_field(stt, "api_key");
    }
    // The webhook HMAC signing key.
    if let Some(webhook) = doc.get_mut("webhook").and_then(|s| s.as_table_like_mut()) {
        mask_field(webhook, "hmac_secret");
    }
    // Each playbook entry carries its own LLM key (`playbook[].llm.api_key`).
    if let Some(arr) = doc
        .get_mut("playbook")
        .and_then(|p| p.as_array_of_tables_mut())
    {
        for entry in arr.iter_mut() {
            if let Some(llm) = entry.get_mut("llm").and_then(|l| l.as_table_like_mut()) {
                mask_field(llm, "api_key");
            }
        }
    }
}

/// Replace a non-empty string `field` in `table` with the redaction marker.
fn mask_field(table: &mut dyn toml_edit::TableLike, field: &str) {
    if let Some(item) = table.get_mut(field) {
        let is_nonempty_secret = item.as_str().is_some_and(|s| !s.is_empty());
        if is_nonempty_secret {
            *item = toml_edit::value(REDACTED_SECRET);
        }
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

    // Persist the SERIALIZED validated Config, not the hand-edited toml_edit
    // doc: re-serializing runs the secret serializer (serialize_secret_string →
    // DPAPI protect), so a freshly-set `whisper.api_key sk-live-…` lands as
    // `dpapi:v1:…` instead of cleartext, and pre-existing encrypted keys stay
    // encrypted. Writing `doc.to_string()` here (the old behavior) bypassed
    // that and wrote secrets in plaintext. Cost: toml_edit's comment/format
    // preservation is dropped — fine for this generated file.
    let body = toml::to_string_pretty(&updated)
        .map_err(|e| format!("failed to serialize updated config: {e}"))?;

    // Atomic replace: write a sibling tmp file, then rename over the target.
    // A crash mid-write leaves the old config intact instead of a truncated
    // half-file the daemon can no longer parse.
    let tmp_path = config_path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, body).map_err(|e| format!("failed to write config: {e}"))?;
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

    /// Serializes every `PHONEME_CONFIG` mutation. `set_var` is process-global,
    /// so under the parallel test runner two tests pointing the env at their own
    /// temp file clobbered each other (a test then wrote/read the wrong path).
    /// Holding this lock for the override's whole lifetime makes the env mutation
    /// effectively single-threaded regardless of `--test-threads`.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Point `PHONEME_CONFIG` at a temp file for the duration of one test,
    /// restoring the previous value on drop so tests can't leak into each other.
    struct ConfigEnvOverride {
        prev: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl ConfigEnvOverride {
        fn set(path: &std::path::Path) -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var("PHONEME_CONFIG").ok();
            std::env::set_var("PHONEME_CONFIG", path);
            Self {
                prev,
                _guard: guard,
            }
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
    fn set_value_does_not_write_secrets_in_plaintext() {
        // Regression for the trust-boundary BLOCKER: `config set` used to write
        // the raw toml_edit doc verbatim, so a freshly-set API key landed
        // cleartext, bypassing the DPAPI secret serializer. The fix persists the
        // serialized validated Config instead (→ serialize_secret_string →
        // protect), so on Windows the key is stored as `dpapi:v1:…` and never
        // raw.
        let tmp = tempfile::tempdir().unwrap();
        let override_path = tmp.path().join("override.toml");
        let _env = ConfigEnvOverride::set(&override_path);

        let cfg = Config::default();
        let secret = "sk-live-xyz";
        set_value(&cfg, "whisper.api_key", secret).expect("set succeeds");

        let written = std::fs::read_to_string(&override_path).expect("config written");

        // On Windows DPAPI encrypts the key, so the raw value must never appear.
        // Off Windows `protect` is a no-op (it can't lose a key on a platform
        // without DPAPI), so only assert the secret round-trips through the
        // serializer — the path that was being bypassed.
        #[cfg(windows)]
        {
            assert!(
                !written.contains(secret),
                "the raw secret must not be written in plaintext: {written}"
            );
            assert!(
                written.contains("dpapi:v1:"),
                "the api_key should be stored DPAPI-encrypted: {written}"
            );
        }

        // Either way, the file round-trips back to the original secret (DPAPI
        // unprotect on Windows; verbatim off Windows) — the fix didn't break
        // read-back.
        let reloaded: Config = toml::from_str(&written).unwrap();
        #[cfg(windows)]
        assert_eq!(reloaded.whisper.api_key_str(), secret);
        // Silence the unused-binding warning off Windows.
        let _ = &reloaded;
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

    #[test]
    fn render_config_redacts_secret_values() {
        // The cross-platform fix: `phoneme config` must not print API keys in
        // cleartext. Off Windows `protect()` is a passthrough, so without this the
        // raw key would land in the terminal/scrollback. A masked dump is safe.
        let mut cfg = Config::default();
        let secret = "sk-live-super-secret-123";
        cfg.whisper.set_api_key(secret.to_owned());

        let rendered = render_config(&cfg, false).expect("render succeeds");

        assert!(
            !rendered.contains(secret),
            "the raw secret must not appear in the dump:\n{rendered}"
        );
        assert!(
            rendered.contains(REDACTED_SECRET),
            "the redaction marker should replace the secret:\n{rendered}"
        );
    }

    #[test]
    fn render_config_leaves_empty_secrets_and_normal_values_alone() {
        // An unset key isn't a secret — masking a blank only obscures that it's
        // empty. Non-secret values must round-trip untouched.
        let cfg = Config::default();
        let rendered = render_config(&cfg, false).expect("render succeeds");

        // The default whisper.api_key is empty, so no marker is forced in.
        assert!(
            !rendered.contains(REDACTED_SECRET),
            "a config with no secrets set should carry no redaction marker:\n{rendered}"
        );
        // And it still parses back to a valid Config (we only edited string
        // values, never the structure).
        let _: Config = toml::from_str(&rendered).expect("rendered TOML round-trips");
    }

    #[test]
    fn render_config_show_secrets_does_not_redact() {
        // The `--show-secrets` opt-out: the dump must NOT carry the redaction
        // marker, so the real value (cleartext off Windows, DPAPI ciphertext on
        // it) is shown verbatim. Asserting the absence of the marker keeps this
        // platform-independent.
        let mut cfg = Config::default();
        cfg.whisper
            .set_api_key("sk-live-super-secret-123".to_owned());

        let rendered = render_config(&cfg, true).expect("render succeeds");

        assert!(
            !rendered.contains(REDACTED_SECRET),
            "--show-secrets must not redact:\n{rendered}"
        );
    }
}
