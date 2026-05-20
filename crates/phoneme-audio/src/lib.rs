//! phoneme-audio — audio capture and WAV encoding for Phoneme.

pub mod format;
pub mod wav;

pub use format::{AudioConfig, Channels, SampleRate};
