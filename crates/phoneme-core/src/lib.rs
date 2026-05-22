//! phoneme-core — shared library for the Phoneme voice notes app.

pub mod catalog;
pub mod config;
pub mod error;
pub mod hook;
pub mod id;
pub mod queue;
pub mod transcription;
pub mod types;
pub mod tags;
pub mod webhook;

pub use catalog::Catalog;
pub use config::Config;
pub use error::{Error, Result};
pub use hook::{HookResult, HookRunner};
pub use id::RecordingId;
pub use queue::{InboxCounts, InboxQueue, InboxState};
pub use tags::Tag;
pub use transcription::TranscriptionClient;
pub use types::{HookMetadata, HookPayload, ListFilter, RecordMode, Recording, RecordingStatus};
