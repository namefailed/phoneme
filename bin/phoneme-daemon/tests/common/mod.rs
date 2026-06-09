//! Shared test harness for daemon integration tests.

use phoneme_ipc::NamedPipeTransport;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use tokio::process::{Child, Command};
use wiremock::matchers::{method, path as wm_path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[allow(dead_code)]
pub struct DaemonHarness {
    pub temp: TempDir,
    pub pipe_name: String,
    pub whisper: MockServer,
    pub client: NamedPipeTransport,
    pub daemon: Child,
}

impl DaemonHarness {
    #[allow(dead_code)]
    pub async fn start() -> Self {
        Self::start_with(|_cfg| {}).await
    }

    /// Like [`start`], but lets the caller mutate the generated `Config` before
    /// the daemon is spawned — e.g. to set hook commands, `run_on_transcribe`,
    /// or keyword rules for hook-behaviour tests.
    #[allow(dead_code)]
    pub async fn start_with<F>(customize: F) -> Self
    where
        F: FnOnce(&mut phoneme_core::Config),
    {
        let temp = TempDir::new().unwrap();
        let pipe_name = format!("phoneme-test-{}", uuid_like());

        // Stub whisper-server.
        let whisper = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wm_path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "hello"})),
            )
            .mount(&whisper)
            .await;

        // Generate a config that points at our stub. External mode makes the
        // daemon use `external_url` (our mock) instead of spawning a bundled
        // whisper binary, so transcription works in CI without a real
        // whisper-server present.
        let mut cfg = phoneme_core::Config::default();
        cfg.whisper.mode = phoneme_core::config::WhisperMode::External;
        cfg.whisper.external_url = whisper.uri();
        cfg.recording.audio_dir = temp.path().join("audio").to_string_lossy().into_owned();
        cfg.recording.silence_threshold_dbfs = -100.0;
        cfg.recording.silence_window_ms = 100;
        // Disable idle pre-roll in tests: it opens a real microphone via cpal,
        // which crashes (STATUS_ACCESS_VIOLATION) on a headless CI runner with no
        // audio endpoint. Tests must never touch real capture hardware.
        cfg.recording.pre_roll_ms = 0;
        cfg.hook.commands.clear();
        cfg.daemon.pipe_name = pipe_name.clone();
        customize(&mut cfg);
        let cfg_path = temp.path().join("config.toml");
        std::fs::write(&cfg_path, toml::to_string(&cfg).unwrap()).unwrap();

        // Spawn the daemon binary.
        let binary = env!("CARGO_BIN_EXE_phoneme-daemon");
        let daemon = Command::new(binary)
            .arg("--foreground")
            .env("PHONEME_CONFIG", &cfg_path)
            .env("PHONEME_DATA_LOCAL", temp.path().join("data"))
            .spawn()
            .unwrap();

        // Wait for the pipe to come up.
        let client = wait_for_client(&pipe_name, Duration::from_secs(10)).await;

        Self {
            temp,
            pipe_name,
            whisper,
            client,
            daemon,
        }
    }

    #[allow(dead_code)]
    pub fn data_local(&self) -> PathBuf {
        self.temp.path().join("data")
    }

    #[allow(dead_code)]
    pub fn audio_dir(&self) -> PathBuf {
        self.temp.path().join("audio")
    }
}

impl Drop for DaemonHarness {
    fn drop(&mut self) {
        let _ = self.daemon.start_kill();
    }
}

async fn wait_for_client(name: &str, total: Duration) -> NamedPipeTransport {
    let start = std::time::Instant::now();
    while start.elapsed() < total {
        match NamedPipeTransport::connect(name).await {
            Ok(c) => return c,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        }
    }
    panic!("daemon never came up on pipe {name}");
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{n:x}")
}
