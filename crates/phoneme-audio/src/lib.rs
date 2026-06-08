//! phoneme-audio — audio capture and WAV encoding for Phoneme.

pub mod convert;
pub mod decode;
pub mod device;
pub mod format;
pub mod meeting_align;
pub mod preroll;
pub mod recorder;
pub mod silence;
pub mod source;
pub mod wav;

pub use decode::{decode_to_canonical_wav, is_supported_extension, SUPPORTED_EXTENSIONS};
pub use device::{default_input_device, list_input_devices, DeviceInfo};
pub use format::{AudioConfig, Channels, SampleRate};
pub use meeting_align::{
    align_meeting_track_samples, align_meeting_tracks, ms_to_samples, AlignedTrack, TrackAlignInput,
};
pub use preroll::PreRollBuffer;
pub use recorder::{Recorder, RecorderConfig, RecordingMode, RecordingResult};
pub use silence::SilenceDetector;
pub use source::{GeneratorSource, SampleBlock, Source, SyntheticSink, SyntheticSource};
