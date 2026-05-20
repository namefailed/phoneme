//! phoneme-core — shared library for the Phoneme voice notes app.

pub mod config;
pub mod error;
pub mod id;
pub mod types;

pub use config::Config;
pub use error::{Error, Result};
pub use id::RecordingId;
pub use types::{HookMetadata, HookPayload, ListFilter, Recording, RecordMode, RecordingStatus};
