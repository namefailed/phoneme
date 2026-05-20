//! phoneme-core — shared library for the Phoneme voice notes app.

pub mod error;
pub mod id;

pub use error::{Error, Result};
pub use id::RecordingId;
