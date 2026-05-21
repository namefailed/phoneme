//! AppState — central holder for all long-lived daemon components.

use crate::event_bus::EventBus;
use phoneme_core::{Catalog, Config, InboxQueue};
use std::path::PathBuf;
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
        let dirs = directories::ProjectDirs::from("", "", "phoneme")
            .ok_or_else(|| anyhow::anyhow!("could not resolve project directories"))?;
        let data_local = dirs.data_local_dir();

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
    pub config: Arc<Config>,
    pub paths: Arc<ResolvedPaths>,
    pub catalog: Catalog,
    pub inbox: InboxQueue,
    pub events: EventBus,
}

impl AppState {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let paths = ResolvedPaths::from_config(&config)?;
        tokio::fs::create_dir_all(&paths.audio_dir).await?;
        tokio::fs::create_dir_all(&paths.inbox_dir).await?;
        tokio::fs::create_dir_all(&paths.log_dir).await?;
        if let Some(parent) = paths.catalog_db.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let catalog = Catalog::open(&paths.catalog_db).await?;
        let inbox = InboxQueue::new(&paths.inbox_dir).await?;

        Ok(Self {
            config: Arc::new(config),
            paths: Arc::new(paths),
            catalog,
            inbox,
            events: EventBus::new(),
        })
    }
}
