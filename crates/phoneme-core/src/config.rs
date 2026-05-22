use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub llm: LlmConfig,
    pub recording: RecordingConfig,
    pub hook: HookConfig,
    pub hotkey: HotkeyConfig,
    pub tray: TrayConfig,
    pub daemon: DaemonConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmMode {
    External,
    BundledModel,
    BundledDownload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmConfig {
    pub mode: LlmMode,
    pub external_url: String,
    pub model_path: String,
    pub bundled_server_port: u16,
    pub bundled_server_args: Vec<String>,
    pub timeout_secs: u64,
    pub system_prompt: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingConfig {
    pub audio_dir: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub silence_threshold_dbfs: f32,
    pub silence_window_ms: u32,
    pub max_duration_secs: u32,
    pub input_device: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookConfig {
    #[serde(alias = "command", deserialize_with = "deserialize_string_or_vec")]
    pub commands: Vec<String>,
    pub timeout_secs: u64,
    #[serde(default)]
    pub webhook_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotkeyConfig {
    pub enabled: bool,
    pub combo: String,
    pub mode: HotkeyMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyMode {
    Hold,
    Toggle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrayConfig {
    pub show_on_startup: bool,
    pub minimize_to_tray: bool,
    pub start_at_login: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub log_level: String,
    pub log_max_size_mb: u32,
    pub log_max_files: u32,
    pub pipe_name: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            llm: LlmConfig {
                mode: LlmMode::External,
                external_url: "http://127.0.0.1:5809".into(),
                model_path: String::new(),
                bundled_server_port: 5809,
                bundled_server_args: vec![],
                timeout_secs: 60,
                system_prompt: "Transcribe the user's speech and clean it into a single line."
                    .into(),
            },
            recording: RecordingConfig {
                audio_dir: "%USERPROFILE%/Documents/phoneme/audio".into(),
                sample_rate: 16000,
                channels: 1,
                silence_threshold_dbfs: -45.0,
                silence_window_ms: 3000,
                max_duration_secs: 300,
                input_device: "default".into(),
            },
            hook: HookConfig {
                commands: vec!["powershell -File %APPDATA%/phoneme/hooks/to-stdout.ps1".into()],
                timeout_secs: 30,
                webhook_url: None,
            },
            hotkey: HotkeyConfig {
                enabled: false,
                combo: "Ctrl+Alt+Space".into(),
                mode: HotkeyMode::Hold,
            },
            tray: TrayConfig {
                show_on_startup: true,
                minimize_to_tray: true,
                start_at_login: false,
            },
            daemon: DaemonConfig {
                log_level: "info".into(),
                log_max_size_mb: 10,
                log_max_files: 5,
                pipe_name: "phoneme-daemon".into(),
            },
        }
    }
}

impl Config {
    /// Load and parse a config file from disk.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let cfg: Self = toml::from_str(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validate constraints not enforced by the type system.
    pub fn validate(&self) -> Result<()> {
        if self.recording.sample_rate < 8000 || self.recording.sample_rate > 96000 {
            return Err(Error::InvalidConfig(format!(
                "recording.sample_rate must be between 8000 and 96000 (got {})",
                self.recording.sample_rate
            )));
        }
        if !(1..=2).contains(&self.recording.channels) {
            return Err(Error::InvalidConfig(format!(
                "recording.channels must be 1 or 2 (got {})",
                self.recording.channels
            )));
        }
        if self.llm.mode == LlmMode::BundledModel && self.llm.model_path.is_empty() {
            return Err(Error::InvalidConfig(
                "llm.model_path is required when llm.mode = bundled_model".into(),
            ));
        }
        match self.daemon.log_level.as_str() {
            "error" | "warn" | "info" | "debug" | "trace" => {}
            other => {
                return Err(Error::InvalidConfig(format!(
                    "daemon.log_level must be error|warn|info|debug|trace (got {other})"
                )));
            }
        }
        Ok(())
    }

    /// Expand `~` and `%VAR%` in user-configurable path fields. Returns a new
    /// Config; original is unchanged.
    pub fn expanded(&self) -> Result<Self> {
        let mut out = self.clone();
        out.recording.audio_dir = expand(&out.recording.audio_dir)?;
        out.llm.model_path = expand(&out.llm.model_path)?;
        let mut expanded_cmds = Vec::new();
        for c in &out.hook.commands {
            expanded_cmds.push(expand(c)?);
        }
        out.hook.commands = expanded_cmds;
        Ok(out)
    }
}

fn expand(s: &str) -> Result<String> {
    if s.is_empty() {
        return Ok(s.into());
    }
    let expanded = shellexpand::full(s)
        .map_err(|e| Error::InvalidConfig(format!("path expansion failed for {s}: {e}")))?;
    Ok(expanded.into_owned())
}

fn deserialize_string_or_vec<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrVec {
        String(String),
        Vec(Vec<String>),
    }

    match StringOrVec::deserialize(deserializer)? {
        StringOrVec::String(s) => Ok(vec![s]),
        StringOrVec::Vec(v) => Ok(v),
    }
}

/// Helper for tests/wizard: resolve the default config file path.
pub fn default_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "phoneme").map(|p| p.config_dir().join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_config(dir: &TempDir, contents: &str) -> PathBuf {
        let path = dir.path().join("config.toml");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn defaults_round_trip_through_toml() {
        let cfg = Config::default();
        let s = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&s).unwrap();
        assert_eq!(parsed, cfg);
    }

    #[test]
    fn defaults_validate() {
        Config::default().validate().expect("defaults are valid");
    }

    #[test]
    fn loads_minimal_valid_config() {
        let dir = TempDir::new().unwrap();
        let cfg_text = toml::to_string(&Config::default()).unwrap();
        let path = write_config(&dir, &cfg_text);
        let cfg = Config::load(&path).expect("loads");
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn rejects_bad_sample_rate() {
        let mut cfg = Config::default();
        cfg.recording.sample_rate = 100;
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, Error::InvalidConfig(_)));
        assert!(format!("{err}").contains("sample_rate"));
    }

    #[test]
    fn rejects_bad_log_level() {
        let mut cfg = Config::default();
        cfg.daemon.log_level = "loud".into();
        let err = cfg.validate().unwrap_err();
        assert!(format!("{err}").contains("log_level"));
    }

    #[test]
    fn bundled_model_requires_model_path() {
        let mut cfg = Config::default();
        cfg.llm.mode = LlmMode::BundledModel;
        cfg.llm.model_path = String::new();
        let err = cfg.validate().unwrap_err();
        assert!(format!("{err}").contains("model_path"));
    }

    #[test]
    fn invalid_toml_returns_toml_error() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "not = valid = toml");
        let err = Config::load(&path).unwrap_err();
        assert!(matches!(err, Error::Toml(_)));
    }

    #[test]
    fn tilde_expansion_in_audio_dir() {
        let mut cfg = Config::default();
        cfg.recording.audio_dir = "~/test".into();
        let expanded = cfg.expanded().unwrap();
        assert!(!expanded.recording.audio_dir.starts_with('~'));
        assert!(
            expanded.recording.audio_dir.ends_with("/test")
                || expanded.recording.audio_dir.ends_with("\\test")
        );
    }
}
