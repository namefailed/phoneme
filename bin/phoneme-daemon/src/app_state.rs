//! AppState — central holder for all long-lived daemon components.

use crate::event_bus::EventBus;
use crate::recorder::DaemonRecorder;
use crate::shutdown::ShutdownCoordinator;
use arc_swap::ArcSwap;
use phoneme_core::{
    webhook::WebhookClient, Catalog, Config, InboxQueue, LlmPostProcessor, Transcriber,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;

/// Resolved paths derived from `Config`. Created once at startup.
#[derive(Debug, Clone)]
pub struct ResolvedPaths {
    pub audio_dir: PathBuf,
    pub inbox_dir: PathBuf,
    pub catalog_db: PathBuf,
    pub log_dir: PathBuf,
}

impl ResolvedPaths {
    pub fn from_config(cfg: &Config) -> anyhow::Result<Self> {
        // PHONEME_DATA_LOCAL lets integration tests redirect inbox/catalog/log
        // away from the real per-user `AppData\Local\phoneme` so concurrent
        // test daemons don't stomp on each other (or on a real install).
        let data_local: PathBuf = if let Ok(p) = std::env::var("PHONEME_DATA_LOCAL") {
            p.into()
        } else {
            let dirs = directories::ProjectDirs::from("", "", "phoneme")
                .ok_or_else(|| anyhow::anyhow!("could not resolve project directories"))?;
            dirs.data_local_dir().to_path_buf()
        };

        // Expand user-config paths.
        let expanded = cfg.expanded()?;
        let audio_dir: PathBuf = expanded.recording.audio_dir.into();

        Ok(Self {
            audio_dir,
            inbox_dir: data_local.join("inbox"),
            catalog_db: data_local.join("catalog.db"),
            log_dir: data_local.join("logs"),
        })
    }
}

/// Central component holder. Cloning `AppState` clones the `Arc` —
/// underlying components are shared.
//
// `catalog` and `inbox` aren't read yet — they're consumed by the IPC
// handlers (Task 5+) and the transcription worker (Task 8). The allow
// silences dead_code until those tasks land.
#[allow(dead_code)]
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<ArcSwap<Config>>,
    pub paths: Arc<ResolvedPaths>,
    pub catalog: Catalog,
    pub inbox: InboxQueue,
    pub events: EventBus,
    pub recorder: DaemonRecorder,
    /// Shared shutdown coordinator. The IPC `Shutdown` handler and `main`
    /// both reference this one instance so `phoneme daemon stop` actually
    /// stops the daemon.
    pub shutdown: Arc<ShutdownCoordinator>,
    /// Shared transcription HTTP client for the whole process. Holds the warm
    /// connection pool and mints a per-recording `TranscriptionProvider` from
    /// the live config, so the pool is reused instead of rebuilt per recording.
    /// Also owns the lazily-loaded local diarization pipeline cache (loaded on
    /// the first diarized recording, reused after); the config-apply paths
    /// (ReloadConfig, the queue worker's post-run reload) invalidate it via
    /// `diarizer_cache()` when `[diarization]` changes.
    pub transcription: Transcriber,
    /// Shared LLM post-processing client. Like `transcription`, holds a warm
    /// connection pool and mints an `LlmProvider` per run from the live config.
    pub llm: LlmPostProcessor,
    pub webhook: WebhookClient,
    pub embedder: Arc<tokio::sync::RwLock<Option<Arc<phoneme_core::Embedder>>>>,
    /// Serializes access to the single (serial) whisper-server. The final
    /// transcription pipeline acquires this permit (waiting if needed); the
    /// streaming preview only runs a tick if it can acquire it *without*
    /// waiting, and otherwise skips. This guarantees the heavy final
    /// transcription is never starved by a flood of preview requests — the bug
    /// that caused "Whisper timed out after 60s" on long recordings while the
    /// preview hammered the server with a big model.
    pub whisper_sem: Arc<tokio::sync::Semaphore>,
    /// The currently-processing recording and its cancellation token, set by the
    /// queue worker around each `pipeline::run` call and cleared after. The
    /// `CancelProcessing` IPC cancels this token to abort the in-flight item.
    pub processing: Arc<
        std::sync::Mutex<
            Option<(
                phoneme_core::RecordingId,
                tokio_util::sync::CancellationToken,
            )>,
        >,
    >,
    /// A ONE-JOB-SCOPED override of the bundled whisper-server's model file,
    /// used by a model-override re-transcription. `None` (the default) means the
    /// supervisor runs the configured `whisper.model_path`.
    ///
    /// Why this exists instead of mutating the global config: a re-transcribe
    /// with a different model must load that model for *only that one job*. The
    /// previous approach wrote the model into the process-global config, which
    /// the whisper supervisor independently polls and restarts on — and which
    /// the queue worker then reverted with a blanket post-run reload, causing a
    /// SECOND restart. Queued/preview transcriptions reading the same global
    /// config raced the flapping server and mass-failed (#49). The override is
    /// applied here, read by the supervisor as the authoritative model, and the
    /// pipeline drives a single serialized restart-to-override / restore cycle
    /// under `whisper_sem` so nothing else touches the server mid-swap.
    pub whisper_model_override: Arc<WhisperModelOverride>,
    /// Per-recording REQUESTED whisper model overrides, keyed by recording id.
    /// The `RetranscribeRecording` handler records the request here at enqueue
    /// time; the pipeline removes and applies it when that job actually runs
    /// (serialized behind the queue + `whisper_sem`), so an override never takes
    /// effect while a *different* recording is mid-transcription. In-memory and
    /// ephemeral — a daemon restart drops pending overrides (the job then re-runs
    /// with the configured model), mirroring the prior behavior where the
    /// global-config override didn't survive a restart either.
    pub pending_overrides:
        Arc<std::sync::Mutex<std::collections::HashMap<phoneme_core::RecordingId, String>>>,
    /// The ports the bundled whisper-servers are ACTUALLY listening on,
    /// published by the supervisors on every (re)spawn. The configured
    /// `bundled_server_port`s are preferences: when a foreign process already
    /// holds one (the startup sweep kills every whisper-server on the box, so
    /// a squatter is never ours), the supervisor starts the server on a free
    /// OS-assigned port and records it here. Consumers resolve
    /// effective-port-or-config via [`WhisperEffectivePorts::apply`] right
    /// where they build a provider — the same flow-daemon-state-into-core
    /// pattern as `whisper_model_override`, so phoneme-core itself stays
    /// daemon-agnostic.
    pub whisper_ports: Arc<WhisperEffectivePorts>,
    /// Explicit whisper-server restart requests (the Doctor's "Fix"). Both
    /// supervisors select on this and bounce their child with the backoff
    /// reset — the path that heals a HUNG server, which the exit-based
    /// auto-restart can't see.
    pub whisper_restart: Arc<tokio::sync::Notify>,
    /// "Skip the current step" requests from the queue UI. The in-flight LLM
    /// stage (cleanup / summary / tagging) races this and aborts when it fires;
    /// the pipeline then continues with the next step, exactly as if that one
    /// stage had failed non-fatally.
    pub skip_stage: Arc<tokio::sync::Notify>,
    /// The daemon's kill-on-close Job Object. Every child this daemon spawns
    /// (whisper-server main + preview, an Owned Ollama) is assigned to it, so
    /// the kernel reaps them all even when the daemon dies uncleanly (panic,
    /// Task Manager). `None` when creation failed — children then rely on the
    /// graceful shutdown paths alone.
    #[cfg(windows)]
    pub job: Option<Arc<phoneme_core::job::KillOnCloseJob>>,
    /// On-demand local Ollama launcher + ownership ledger. LLM steps call
    /// `ollama_launcher::ensure_ready` through it right before they run; the
    /// shutdown path calls `shutdown()` to stop an Owned (daemon-launched)
    /// Ollama while leaving a user-started one untouched.
    pub ollama: Arc<crate::ollama_launcher::OllamaLauncher>,
}

/// Coordination cell between a model-override re-transcription (in the pipeline)
/// and the whisper supervisor. The supervisor treats `get()` (when `Some`) as
/// the model file to run, falling back to the configured `model_path`; the
/// `changed` notify lets the supervisor react to a set/clear without waiting out
/// its poll interval.
#[derive(Default)]
pub struct WhisperModelOverride {
    /// The override model path, or `None` to use the configured model.
    model: std::sync::Mutex<Option<String>>,
    /// Pinged whenever `model` is set or cleared so the supervisor can restart
    /// the server promptly instead of waiting for its next 1s poll tick.
    pub changed: tokio::sync::Notify,
}

impl WhisperModelOverride {
    /// Current override model path, if any.
    pub fn get(&self) -> Option<String> {
        self.model.lock().unwrap().clone()
    }

    /// Set (`Some`) or clear (`None`) the override and wake the supervisor.
    pub fn set(&self, value: Option<String>) {
        *self.model.lock().unwrap() = value;
        self.changed.notify_waiters();
    }
}

/// Live listen ports of the two bundled whisper-servers. `None` = that server
/// is not running (or its port is unknown), in which case consumers fall back
/// to the configured port. Written by the supervisors right before each spawn
/// and cleared when they idle (external mode, preview not needed, missing
/// binary/model, child exit) — publishing *before* the spawn is what lets each
/// sibling's pre-flight probe exclude the other's port even while that server
/// is mid-restart and momentarily unbound.
///
/// 0 is the "unset" sentinel: a real listener can never bind port 0, so the
/// atomics need no separate validity flag.
#[derive(Debug, Default)]
pub struct WhisperEffectivePorts {
    /// The main (final-transcription) server's port; 0 = not running.
    main: AtomicU16,
    /// The dedicated live-preview server's port; 0 = not running.
    preview: AtomicU16,
}

impl WhisperEffectivePorts {
    /// The main server's live port, when it is running.
    pub fn main(&self) -> Option<u16> {
        match self.main.load(Ordering::Relaxed) {
            0 => None,
            p => Some(p),
        }
    }

    /// The preview server's live port, when it is running.
    pub fn preview(&self) -> Option<u16> {
        match self.preview.load(Ordering::Relaxed) {
            0 => None,
            p => Some(p),
        }
    }

    /// Publish (`Some`) or clear (`None`) the main server's live port.
    pub fn set_main(&self, port: Option<u16>) {
        self.main.store(port.unwrap_or(0), Ordering::Relaxed);
    }

    /// Publish (`Some`) or clear (`None`) the preview server's live port.
    pub fn set_preview(&self, port: Option<u16>) {
        self.preview.store(port.unwrap_or(0), Ordering::Relaxed);
    }

    /// The port consumers should dial for `provider`: the matching server's
    /// published live port when there is one, else the configured port.
    ///
    /// Matching is by preferred port because `provider` may be `[whisper]`
    /// itself, `[preview_whisper]`, or an `[in_place].stt` block the Settings
    /// UI pointed at either server's configured port — all three must follow
    /// the same server wherever it actually landed.
    pub fn resolve(&self, cfg: &Config, provider: &phoneme_core::config::WhisperConfig) -> u16 {
        let preferred = provider.bundled_server_port;
        if preferred == cfg.whisper.bundled_server_port {
            if let Some(p) = self.main() {
                return p;
            }
        } else if cfg
            .preview_whisper
            .as_ref()
            .is_some_and(|p| p.bundled_server_port == preferred)
        {
            if let Some(p) = self.preview() {
                return p;
            }
        }
        preferred
    }

    /// Clone `provider` with its preferred port swapped for the live one, so
    /// `server_base_url()` on the result names the server that is actually
    /// listening. Only a local bundled server is rewritten — external
    /// endpoints are user-managed and cloud backends never use the port.
    pub fn apply(
        &self,
        cfg: &Config,
        provider: &phoneme_core::config::WhisperConfig,
    ) -> phoneme_core::config::WhisperConfig {
        use phoneme_core::config::{TranscriptionBackend, WhisperMode};
        let mut out = provider.clone();
        if out.provider == TranscriptionBackend::Local
            && matches!(
                out.mode,
                WhisperMode::BundledModel | WhisperMode::BundledDownload
            )
        {
            out.bundled_server_port = self.resolve(cfg, provider);
        }
        out
    }
}

static INIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl AppState {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let paths = {
            let _guard = INIT_LOCK.lock().unwrap();
            ResolvedPaths::from_config(&config)?
        };
        tokio::fs::create_dir_all(&paths.audio_dir).await?;
        tokio::fs::create_dir_all(&paths.inbox_dir).await?;
        tokio::fs::create_dir_all(&paths.log_dir).await?;
        if let Some(parent) = paths.catalog_db.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let catalog = Catalog::open(&paths.catalog_db).await?;
        let inbox = InboxQueue::new(&paths.inbox_dir).await?;
        let transcription = Transcriber::new()?;
        let llm = LlmPostProcessor::new()?;
        let webhook = WebhookClient::new()?;

        let embedder = if config.semantic_search.enabled {
            match phoneme_core::Embedder::new(&config.semantic_search) {
                Ok(e) => Some(Arc::new(e)),
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to load semantic search model");
                    None
                }
            }
        } else {
            None
        };

        // The OS-level child reaper. Children are assigned at spawn time
        // (whisper supervisors, the Ollama launcher); failing to create it
        // only loses the unclean-death safety net, never normal operation.
        #[cfg(windows)]
        let job = match phoneme_core::job::KillOnCloseJob::new() {
            Ok(j) => Some(Arc::new(j)),
            Err(e) => {
                tracing::warn!(error = %e, "could not create the daemon job object; children may outlive an unclean daemon death");
                None
            }
        };
        #[cfg(windows)]
        let ollama = Arc::new(crate::ollama_launcher::OllamaLauncher::new(job.clone()));
        #[cfg(not(windows))]
        let ollama = Arc::new(crate::ollama_launcher::OllamaLauncher::new());

        Ok(Self {
            config: Arc::new(ArcSwap::from_pointee(config)),
            paths: Arc::new(paths),
            catalog,
            inbox,
            events: EventBus::new(),
            recorder: DaemonRecorder::default(),
            shutdown: Arc::new(ShutdownCoordinator::new()),
            transcription,
            llm,
            webhook,
            embedder: Arc::new(tokio::sync::RwLock::new(embedder)),
            whisper_sem: Arc::new(tokio::sync::Semaphore::new(1)),
            processing: Arc::new(std::sync::Mutex::new(None)),
            whisper_model_override: Arc::new(WhisperModelOverride::default()),
            pending_overrides: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            whisper_ports: Arc::new(WhisperEffectivePorts::default()),
            whisper_restart: Arc::new(tokio::sync::Notify::new()),
            skip_stage: Arc::new(tokio::sync::Notify::new()),
            #[cfg(windows)]
            job,
            ollama,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::WhisperEffectivePorts;
    use phoneme_core::config::{Config, TranscriptionBackend, WhisperMode};

    /// A config with the main server on 5809 and a dedicated local preview
    /// server on 5810 — the documented two-server layout.
    fn two_server_config() -> Config {
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Local;
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.bundled_server_port = 5809;
        let mut pv = cfg.whisper.clone();
        pv.bundled_server_port = 5810;
        cfg.preview_whisper = Some(pv);
        cfg
    }

    #[test]
    fn unpublished_ports_resolve_to_the_configured_ones() {
        // Nothing published (servers not running) → the configured ports are
        // all consumers can dial, exactly the pre-fallback behavior.
        let cfg = two_server_config();
        let ports = WhisperEffectivePorts::default();
        assert_eq!(ports.resolve(&cfg, &cfg.whisper), 5809);
        assert_eq!(
            ports.resolve(&cfg, cfg.preview_whisper.as_ref().unwrap()),
            5810
        );
    }

    #[test]
    fn url_derivation_prefers_the_effective_port_over_config() {
        // The supervisor fell back from 5809 to an OS-assigned port; the URL
        // every consumer derives must name the live port, not the config.
        let cfg = two_server_config();
        let ports = WhisperEffectivePorts::default();
        ports.set_main(Some(51234));
        let effective = ports.apply(&cfg, &cfg.whisper);
        assert_eq!(effective.bundled_server_port, 51234);
        assert_eq!(effective.server_base_url(), "http://127.0.0.1:51234");
        // Clearing it (server stopped) falls back to the configured port.
        ports.set_main(None);
        assert_eq!(
            ports.apply(&cfg, &cfg.whisper).server_base_url(),
            "http://127.0.0.1:5809"
        );
    }

    #[test]
    fn preview_config_follows_the_preview_servers_port() {
        // Each provider config maps to ITS server: a preview fallback must
        // never redirect the main config, and vice versa.
        let cfg = two_server_config();
        let ports = WhisperEffectivePorts::default();
        ports.set_main(Some(51234));
        ports.set_preview(Some(52345));
        assert_eq!(ports.resolve(&cfg, &cfg.whisper), 51234);
        assert_eq!(
            ports.resolve(&cfg, cfg.preview_whisper.as_ref().unwrap()),
            52345
        );
    }

    #[test]
    fn in_place_stt_pointing_at_a_servers_port_follows_it() {
        // The Settings UI builds `[in_place].stt` blocks that reuse the main
        // or preview server by configured port; resolution must map those
        // through the same live ports.
        let cfg = two_server_config();
        let ports = WhisperEffectivePorts::default();
        ports.set_preview(Some(52345));
        let mut stt = cfg.whisper.clone();
        stt.bundled_server_port = 5810; // "same server as the preview"
        assert_eq!(ports.resolve(&cfg, &stt), 52345);
        // A port matching neither server is left alone — there is no
        // supervisor publishing a live port for it.
        stt.bundled_server_port = 7000;
        assert_eq!(ports.resolve(&cfg, &stt), 7000);
    }

    #[test]
    fn external_and_cloud_configs_are_never_rewritten() {
        // An external endpoint is user-managed and a cloud backend has no
        // local port — `apply` must leave both byte-identical.
        let cfg = two_server_config();
        let ports = WhisperEffectivePorts::default();
        ports.set_main(Some(51234));

        let mut external = cfg.whisper.clone();
        external.mode = WhisperMode::External;
        external.external_url = "http://10.0.0.7:9000".into();
        let out = ports.apply(&cfg, &external);
        assert_eq!(out.bundled_server_port, 5809);
        assert_eq!(out.server_base_url(), "http://10.0.0.7:9000");

        let mut cloud = cfg.whisper.clone();
        cloud.provider = TranscriptionBackend::Openai;
        assert_eq!(ports.apply(&cfg, &cloud).bundled_server_port, 5809);
    }
}
