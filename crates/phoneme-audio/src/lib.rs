//! phoneme-audio — audio capture and WAV encoding for Phoneme.

pub mod convert;
pub mod device;
pub mod format;
pub mod silence;
pub mod source;
pub mod wav;

pub use device::{default_input_device, list_input_devices, DeviceInfo};
pub use format::{AudioConfig, Channels, SampleRate};
pub use silence::SilenceDetector;
pub use source::{SampleBlock, Source, SyntheticSink, SyntheticSource};
