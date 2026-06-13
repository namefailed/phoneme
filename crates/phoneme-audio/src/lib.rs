//! phoneme-audio — audio capture and WAV encoding for Phoneme.
//!
//! Everything Phoneme records or imports is funneled into one canonical PCM
//! format — 16-bit signed samples, 16 kHz, mono — before it reaches the
//! transcription pipeline. The modules here cover the path from a live capture
//! device (or an imported file) to that canonical WAV: device enumeration
//! ([`device`]), the capture state machine ([`recorder`]) over a pluggable
//! sample [`source`], sample-format conversion ([`convert`]), silence detection
//! for auto-stop ([`silence`]), peak normalization of the finished recording
//! ([`normalize`]), a pre-roll ring buffer ([`preroll`]), WAV read/write
//! ([`wav`]), arbitrary-file decoding ([`decode`]), and wall-clock alignment of
//! meeting-mode dual tracks ([`meeting_align`]).

#![warn(missing_docs)]

pub mod convert;
pub mod decode;
pub mod device;
pub mod format;
pub mod meeting_align;
pub mod normalize;
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
pub use normalize::normalize_peak;
pub use preroll::PreRollBuffer;
pub use recorder::{Recorder, RecorderConfig, RecordingMode, RecordingResult};
pub use silence::SilenceDetector;
pub use source::{GeneratorSource, SampleBlock, Source, SyntheticSink, SyntheticSource};
