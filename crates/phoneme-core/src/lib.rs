//! phoneme-core — shared library for the Phoneme voice notes app.

pub mod catalog;
pub mod chunk;
pub mod config;
pub mod diarization;
pub mod dictation;
pub mod doctor;
pub mod embed;
pub mod error;
pub mod fusion;
pub mod hook;
pub mod id;
pub mod llm;
#[cfg(feature = "native-whisper")]
pub mod native_whisper;
pub mod profiles;
pub mod queue;
pub(crate) mod secret_crypto;
pub mod tags;
pub mod transcription;
pub mod types;
pub mod webhook;

pub use catalog::Catalog;
pub use chunk::chunk_transcript;
pub use config::Config;
pub use embed::Embedder;
pub use error::{Error, Result};
pub use fusion::{calibrate_cosine, reciprocal_rank_fusion};
pub use hook::{HookResult, HookRunner};
pub use id::RecordingId;
pub use llm::{LlmPostProcessor, LlmProvider};
pub use queue::{InboxCounts, InboxQueue, InboxState};
pub use tags::Tag;
pub use transcription::{
    AssemblyAiProvider, DeepgramProvider, OpenAiCompatProvider, Transcriber, TranscriptionProvider,
};
pub use types::{
    HookMetadata, HookPayload, ListFilter, MeetingTrack, RecordMode, Recording, RecordingStatus,
    SpeakerName, TranscriptSegment,
};
