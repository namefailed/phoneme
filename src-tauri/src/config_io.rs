//! Atomic config.toml read/write.

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
