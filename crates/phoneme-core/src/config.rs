use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The root configuration object for Phoneme.
/// This configuration encapsulates the entire system state, including settings for
/// transcription (Whisper), audio recording parameters, post-processing hooks,
/// keyboard hotkeys, frontend UI theming, and daemon logging.
///
/// It maps directly to the user's `config.toml` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Configuration for the Whisper transcription engine.
    pub whisper: LlmConfig,
    /// Hardware and threshold settings for the audio recording stream.
    pub recording: RecordingConfig,
    /// Settings governing external script execution (hooks) upon transcription success.
    pub hook: HookConfig,
    /// Global keyboard shortcut bindings for triggering push-to-talk.
    pub hotkey: HotkeyConfig,
    /// Frontend OS-level tray behavior.
    pub tray: TrayConfig,
    /// Settings for the built-in transcript editor.
    #[serde(default)]
    pub editor: EditorConfig,
    /// Background daemon runtime settings (e.g., logging verbosity).
    pub daemon: DaemonConfig,
    /// Frontend aesthetics and layout settings.
    #[serde(default)]
    pub interface: InterfaceConfig,
    /// Settings for the optional LLM-powered transcript cleanup/post-processing pipeline.
    #[serde(default = "default_llm_post_process")]
    pub llm_post_process: LlmPostProcessConfig,
}

/// Configures the optional accessibility layer for post-processing transcriptions using an LLM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmPostProcessConfig {
    /// Whether the LLM post-processing step is active.
    pub enabled: bool,
    /// The provider type to use (e.g., `ollama`, `openai`, `none`).
    pub provider: String,
    /// API key for authentication, if required by the chosen provider.
    pub api_key: String,
    /// Base URL for the API. If empty, the provider's default is used.
    #[serde(default)]
    pub api_url: String,
    /// The specific model identifier to target (e.g., `llama3`, `gpt-4o`).
    pub model: String,
    /// The system prompt used to instruct the LLM on how to clean the text.
    pub prompt: String,
}

fn default_llm_post_process() -> LlmPostProcessConfig {
    LlmPostProcessConfig {
        enabled: false,
        provider: "none".into(),
        api_key: "".into(),
        api_url: "".into(),
        model: "llama3.2:3b".into(),
        prompt: "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.".into(),
    }
}

/// Defines the execution strategy for the Whisper transcription model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhisperMode {
    /// Connect to a standalone, externally managed OpenAI-compatible API endpoint.
    External,
    /// Spin up a local `whisper-server` process using a specific model file on disk.
    BundledModel,
    /// Download and run a bundled model seamlessly as part of the first-run experience.
    BundledDownload,
}

/// Configuration for the Whisper transcription engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmConfig {
    /// The execution mode determining how the transcription server is managed.
    pub mode: WhisperMode,
    /// The URL of the OpenAI-compatible transcription endpoint (used in `External` mode).
    pub external_url: String,
    /// The absolute path to the local GGML model file (used in `BundledModel` mode).
    pub model_path: String,
    /// The network port the bundled local server will bind to.
    pub bundled_server_port: u16,
    /// Additional command-line arguments passed to the bundled server on startup.
    pub bundled_server_args: Vec<String>,
    /// The maximum time in seconds to wait for a transcription response before timing out.
    pub timeout_secs: u64,
}

impl LlmConfig {
    /// OpenAI-compatible Whisper server base URL (no trailing path).
    pub fn server_base_url(&self) -> String {
        match self.mode {
            WhisperMode::External => self.external_url.trim_end_matches('/').to_string(),
            WhisperMode::BundledModel | WhisperMode::BundledDownload => {
                format!("http://127.0.0.1:{}", self.bundled_server_port)
            }
        }
    }
}

/// Hardware and threshold settings for the audio recording stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingConfig {
    /// The absolute path to the directory where `.wav` recordings are saved.
    pub audio_dir: String,
    /// The sample rate for recording (e.g., 16000 or 44100).
    pub sample_rate: u32,
    /// The number of audio channels (typically 1 for mono).
    pub channels: u8,
    /// The silence threshold in dBFS (e.g., -45.0). Audio below this is considered silence.
    pub silence_threshold_dbfs: f32,
    /// The duration of contiguous silence (in milliseconds) required to automatically stop recording.
    pub silence_window_ms: u32,
    /// The absolute maximum duration (in seconds) a single recording can last before being forcefully stopped.
    pub max_duration_secs: u32,
    /// The name of the specific input device to record from, or `"default"` to use the system default.
    pub input_device: String,
}

/// Settings governing external script execution (hooks) upon transcription success.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookConfig {
    /// A list of shell commands or script paths to execute sequentially.
    #[serde(alias = "command", deserialize_with = "deserialize_string_or_vec")]
    pub commands: Vec<String>,
    /// The maximum execution time allowed for a hook before it is forcefully killed.
    pub timeout_secs: u64,
    /// An optional HTTP URL where the transcription payload will be POSTed concurrently.
    #[serde(default)]
    pub webhook_url: Option<String>,
}

/// Global keyboard shortcut bindings for triggering push-to-talk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotkeyConfig {
    /// Whether the global hotkey listener is active.
    pub enabled: bool,
    /// The key combination to bind (e.g., `"Ctrl+Alt+Space"`).
    pub combo: String,
    /// The behavioral mode of the hotkey (Hold vs Toggle).
    pub mode: HotkeyMode,
}

/// The behavioral mode of the global recording hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyMode {
    /// Recording only happens while the key combination is physically held down.
    Hold,
    /// Pressing the combination toggles recording on; pressing it again toggles it off.
    Toggle,
}

/// Frontend OS-level tray behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrayConfig {
    /// If true, the main window will automatically open when the app starts.
    pub show_on_startup: bool,
    /// If true, closing the main window simply minimizes the app to the system tray.
    pub minimize_to_tray: bool,
    /// If true, the application registers a Windows run key to start automatically on system login.
    pub start_at_login: bool,
}

/// Frontend aesthetics and layout settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterfaceConfig {
    /// Whether to strip the OS window decorations (title bar).
    #[serde(default)]
    pub strip_titlebar: bool,
    /// If true, use 24-hour time format in the UI.
    #[serde(default)]
    pub format_24h: bool,
    /// The active CSS theme identifier (e.g., `"catppuccin-mocha"`, `"tokyo-night"`).
    #[serde(default = "default_theme")]
    pub theme: String,
    /// A list of column identifiers that are currently visible in the main list view.
    #[serde(default = "default_visible_columns")]
    pub visible_columns: Vec<String>,
    /// Column widths for the main list view.
    #[serde(default = "default_column_widths")]
    pub column_widths: Vec<String>,
}

fn default_column_widths() -> Vec<String> {
    vec![
        "100px".into(),
        "60px".into(),
        "60px".into(),
        "100px".into(),
        "1fr".into(),
    ]
}

/// Settings specifically for the transcript editor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EditorConfig {
    /// Whether the CodeMirror editor uses Vim keybindings.
    #[serde(default)]
    pub vim_mode: bool,
    /// Custom Vimrc settings (like key remappings) applied when vim_mode is enabled.
    #[serde(default)]
    pub vimrc: String,
    /// Absolute path to an external .vimrc file to load automatically.
    #[serde(default)]
    pub vimrc_path: String,
}

fn default_theme() -> String {
    "catppuccin-mocha".into()
}

fn default_visible_columns() -> Vec<String> {
    vec![
        "day".into(),
        "time".into(),
        "duration".into(),
        "status".into(),
        "transcript".into(),
    ]
}

/// Background daemon runtime settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// The verbosity of the daemon's internal log (e.g., `info`, `debug`, `trace`).
    pub log_level: String,
    /// The maximum size in megabytes before the log file is rotated.
    pub log_max_size_mb: u32,
    /// The maximum number of rotated log files to retain before old ones are deleted.
    pub log_max_files: u32,
    /// The Named Pipe (Windows) or Unix Socket path used for IPC communication.
    pub pipe_name: String,
}

impl Default for InterfaceConfig {
    fn default() -> Self {
        Config::default().interface
    }
}

impl Default for TrayConfig {
    fn default() -> Self {
        Config::default().tray
    }
}

impl Config {
    pub fn read_or_default() -> Self {
        default_config_path()
            .and_then(|p| Self::load(&p).ok())
            .unwrap_or_default()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            whisper: LlmConfig {
                mode: WhisperMode::BundledDownload,
                external_url: "http://127.0.0.1:5809".into(),
                model_path: String::new(),
                bundled_server_port: 5809,
                bundled_server_args: vec![],
                timeout_secs: 60,
            },
            recording: RecordingConfig {
                audio_dir: "~/Documents/phoneme/audio".into(),
                sample_rate: 16000,
                channels: 1,
                silence_threshold_dbfs: -45.0,
                silence_window_ms: 3000,
                max_duration_secs: 300,
                input_device: "default".into(),
            },
            hook: HookConfig {
                commands: vec![
                    "powershell -File ~/AppData/Roaming/phoneme/hooks/to-stdout.ps1".into(),
                ],
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
            interface: InterfaceConfig {
                strip_titlebar: false,
                format_24h: false,
                theme: "catppuccin-mocha".into(),
                visible_columns: vec![
                    "day".into(),
                    "time".into(),
                    "duration".into(),
                    "status".into(),
                    "transcript".into(),
                ],
                column_widths: default_column_widths(),
            },
            editor: EditorConfig {
                vim_mode: false,
                vimrc: String::new(),
                vimrc_path: String::new(),
            },
            daemon: DaemonConfig {
                log_level: "info".into(),
                log_max_size_mb: 10,
                log_max_files: 5,
                pipe_name: "phoneme-daemon".into(),
            },
            llm_post_process: LlmPostProcessConfig {
                enabled: false,
                provider: "none".into(),
                api_key: "".into(),
                api_url: "".into(),
                model: "llama3.2:3b".into(),
                prompt: "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.".into(),
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
        if self.whisper.mode == WhisperMode::BundledModel && self.whisper.model_path.is_empty() {
            return Err(Error::InvalidConfig(
                "whisper.model_path is required when whisper.mode = bundled_model".into(),
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
        out.recording.audio_dir = expand_path(&out.recording.audio_dir)?;
        out.whisper.model_path = expand_path(&out.whisper.model_path)?;
        // Hook commands are arbitrary shell strings that may contain $variables
        // used at runtime by the shell (e.g. `$payload`, `$input` in PowerShell).
        // Only expand the known Phoneme path tokens (%APPDATA%, ~/) — do NOT
        // run them through shellexpand::env, which would misinterpret those
        // shell variables as OS environment variable references and error.
        out.hook.commands = out.hook.commands.iter().map(|c| expand_cmd(c)).collect();
        Ok(out)
    }
}

/// Expand `~` and `%VAR%` path tokens in a file-path string, then resolve
/// any remaining `$VAR`-style OS environment variable references via
/// shellexpand. Use this for path-only fields like `audio_dir` and
/// `model_path`.
fn expand_path(s: &str) -> Result<String> {
    if s.is_empty() {
        return Ok(s.into());
    }
    let s = expand_home_tokens(s);
    let expanded = shellexpand::env(&s)
        .map_err(|e| Error::InvalidConfig(format!("path expansion failed for {s}: {e}")))?;
    Ok(expanded.into_owned())
}

/// Expand only the Phoneme-specific path tokens (`%APPDATA%`, `%USERPROFILE%`,
/// `~/`) in a shell command string. Shell-variable references like `$payload`
/// or `$input` are left untouched — they are runtime variables for the hook
/// process, not OS environment variables for Phoneme to resolve.
fn expand_cmd(s: &str) -> String {
    expand_home_tokens(s)
}

/// Replace `%USERPROFILE%`, `%APPDATA%`, and leading `~/` with absolute paths.
fn expand_home_tokens(s: &str) -> String {
    let mut s = s
        .replace("%USERPROFILE%", "~")
        .replace("%APPDATA%", "~/AppData/Roaming");
    if let Some(home_dir) =
        directories::UserDirs::new().map(|u| u.home_dir().to_string_lossy().to_string())
    {
        s = s.replace("~/", &format!("{}/", home_dir.replace('\\', "/")));
    }
    s
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
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.model_path = String::new();
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
    #[test]
    fn parses_legacy_config_without_llm() {
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        // create a config string without llm
        let mut toml_val: toml::Value = toml::Value::try_from(cfg).unwrap();
        toml_val.as_table_mut().unwrap().remove("llm_post_process");
        let cfg_text = toml::to_string(&toml_val).unwrap();

        let path = write_config(&dir, &cfg_text);
        let parsed = Config::load(&path).expect("loads legacy config");
        assert!(!parsed.llm_post_process.enabled);
        assert_eq!(parsed.llm_post_process.provider, "none");
        assert_eq!(parsed.llm_post_process.model, "llama3.2:3b");
    }

    #[test]
    fn parses_interface_configuration() {
        let dir = TempDir::new().unwrap();
        let mut cfg = Config::default();
        cfg.interface.theme = "tokyo-night".to_string();
        cfg.interface.strip_titlebar = true;
        cfg.interface.column_widths = vec!["150px".to_string()];

        let path = dir.path().join("config.toml");
        let toml_str = toml::to_string(&cfg).unwrap();
        std::fs::write(&path, toml_str).unwrap();

        let parsed = Config::load(&path).unwrap();
        assert_eq!(parsed.interface.theme, "tokyo-night");
        assert!(parsed.interface.strip_titlebar);
        assert_eq!(parsed.interface.column_widths.first().unwrap(), "150px");
    }

    /// Regression: a hook command that contains PowerShell `$variables` (e.g.
    /// `$payload`, `$input`) must not cause `expanded()` to fail with "env var
    /// not found". Those are shell runtime variables, not OS env vars.
    #[test]
    fn expanded_does_not_shellexpand_hook_commands() {
        let mut cfg = Config::default();
        // Simulate the clipboard preset: contains $d and $input which are
        // PowerShell variables, NOT environment variables.
        cfg.hook.commands = vec![
            r#"powershell -Command "$d=($input|Out-String|ConvertFrom-Json); Set-Clipboard -Value $d.transcript""#.into(),
        ];
        // Must not return Err — $d is not an env var but should be left alone.
        let expanded = cfg.expanded().expect("hook commands with $vars should not fail expansion");
        // The $-variables must be preserved verbatim (not expanded to empty or error).
        assert!(expanded.hook.commands[0].contains("$d"));
        assert!(expanded.hook.commands[0].contains("$input"));
    }

    /// %APPDATA% in a hook command must still be expanded to an absolute path.
    #[test]
    fn expanded_hook_commands_expand_appdata_token() {
        let mut cfg = Config::default();
        cfg.hook.commands = vec![
            "powershell -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-clipboard.ps1"
                .into(),
        ];
        let expanded = cfg.expanded().expect("expands ok");
        assert!(
            !expanded.hook.commands[0].contains("%APPDATA%"),
            "APPDATA token should be replaced, got: {}",
            expanded.hook.commands[0]
        );
        assert!(
            expanded.hook.commands[0].contains("phoneme/hooks/to-clipboard.ps1"),
            "path suffix should be preserved"
        );
    }
}
