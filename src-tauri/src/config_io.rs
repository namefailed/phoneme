//! Atomic config.toml read/write — the tray's only path to the config file.
//!
//! `read` returns defaults when the file doesn't exist yet (first run);
//! `write` validates first and replaces the file atomically (temp +
//! rename), so a crash mid-save can never leave a truncated config that
//! bricks the daemon's next load. Reads here see real secrets — the masking
//! for the WebView happens a layer up in `commands::read_config`; daemon-
//! and tray-side code that needs actual keys calls this module directly.
//! Uses the per-user default path (the tray doesn't honor `PHONEME_CONFIG`;
//! that override is a CLI/daemon/testing affordance).

use phoneme_core::Config;
use std::path::PathBuf;

pub fn config_path() -> anyhow::Result<PathBuf> {
    phoneme_core::config::default_config_path()
        .ok_or_else(|| anyhow::anyhow!("could not resolve config path"))
}

pub fn read() -> anyhow::Result<Config> {
    let path = config_path()?;
    if path.exists() {
        Ok(Config::load(&path)?)
    } else {
        Ok(Config::default())
    }
}

/// Write the config atomically: temp file → rename.
pub fn write(config: &Config) -> anyhow::Result<()> {
    config.validate()?;
    let path = config_path()?;
    phoneme_core::config::ensure_config_dir()?;
    let body = toml::to_string_pretty(config)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn exists() -> bool {
    config_path().map(|p| p.exists()).unwrap_or(false)
}
