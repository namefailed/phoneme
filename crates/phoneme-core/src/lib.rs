//! phoneme-core — the shared library every Phoneme surface is built on.
//!
//! This crate owns the domain logic and data layer that the daemon, the CLI
//! (`phoneme`), and the tray (`phoneme-tray`) all share. It deliberately knows
//! nothing about IPC, windows, or hotkeys — it transcribes audio, post-processes
//! the text, stores the result, and answers questions about the archive. The
//! daemon wires these pieces into a running pipeline; everything here is the
//! reusable machinery underneath.
//!
//! # The system, by pipeline stage
//!
//! A recording's life runs left to right; the modules group the same way.
//!
//! **Capture → transcribe**
//! - [`config`] — the `config.toml` schema (`Config` and every section). API
//!   keys are encrypted at rest; many sub-configs inherit a blank field from the
//!   cleanup connection. The contract every other stage reads its settings from.
//! - [`transcription`] — turns an audio file into text. One trait, many
//!   backends (local whisper.cpp, OpenAI/Groq, Deepgram, AssemblyAI,
//!   ElevenLabs, any OpenAI-compatible URL); [`Transcriber`] mints the right one
//!   per run and keeps the HTTP pool warm.
//! - `native_whisper` — an in-process whisper-rs backend (feature
//!   `native-whisper`), an alternative to the HTTP server for the local path.
//! - [`diarization`] — local speaker labelling (pyannote via speakrs) plus the
//!   pure logic that maps speaker turns onto transcript segments. Owns the
//!   process-wide pipeline cache so the ~500 MB models load once.
//!
//! **Post-process**
//! - [`llm`] — optional LLM cleanup/summary/tag/title passes over a transcript.
//!   Same shape as `transcription`: one trait, one impl per backend (Ollama,
//!   OpenAI-compatible, Anthropic). All errors are non-fatal — the pipeline
//!   falls back to the raw transcript.
//! - [`dictation`] — the zero-latency rule-based text polish behind the in-place
//!   fast lane (filler stripping, stutter collapse, capitalization).
//! - [`title`] — derive a short display title from a transcript (a pure
//!   heuristic; the optional LLM title is orchestrated by the pipeline).
//! - [`chunk`] — split a transcript into sentence-aware overlapping windows so
//!   each idea embeds on its own tight vector (the paraphrase-recall fix).
//!
//! **Store**
//! - [`catalog`] — the SQLite recordings database: rows, transcripts, segments,
//!   tags, speaker names, embeddings, and every query the UI runs (list,
//!   filter, retention). The durable home of the archive.
//! - [`types`] — the domain types shared across the workspace ([`Recording`],
//!   [`RecordingStatus`], [`TranscriptSegment`], the hook payload, list
//!   filters).
//! - [`id`] — the time-ordered [`RecordingId`] that names every recording (and,
//!   via its date prefix, its file path on disk).
//! - [`queue`] — the filesystem-backed inbox that feeds the daemon's worker:
//!   pending → processing → done/failed as atomic renames, crash-recovery
//!   included.
//! - [`tags`] — the [`Tag`] record (id, name, colour).
//! - [`profiles`] — named full-config snapshots a user can switch between.
//!
//! **Recall (search & retrieval)**
//! - [`embed`] — the ONNX sentence-transformer that turns text into vectors for
//!   semantic search.
//! - [`fusion`] — Reciprocal Rank Fusion (blend the vector and keyword
//!   rankings) plus cosine→percentage calibration for the relevance the UI
//!   shows.
//!
//! **Side effects & egress**
//! - [`hook`] — run a user's hook subprocess with the transcript on stdin, with
//!   timeout, output draining, and secret redaction for the hook-test path.
//! - [`webhook`] — POST the transcript to a URL, behind an SSRF guard that
//!   keeps loopback open, gates the LAN, and requires TLS for the public net.
//!
//! **Operational**
//! - [`doctor`] — provider-aware health checks (model files, disk space,
//!   endpoint reachability, API-key presence) shared by the GUI and the CLI.
//! - [`error`] — the crate's single [`Error`](enum@Error) type, mapped 1:1 to
//!   the IPC error kinds so the daemon forwards failures without translation.
//! - [`job`] (Windows only) — a kill-on-close Job Object: the OS-level safety
//!   net that reaps spawned children when their owner dies.
//!
//! **Dev / eval metrics** (not wired into the pipeline)
//! - [`der`] — Diarization Error Rate: missed + false-alarm + confusion /
//!   total reference speech. Scored against RTTM reference files.
//! - [`voiceprint_eval`] — EER calibration for the named-speaker recognizer:
//!   genuine vs impostor FAR/FRR sweep to pick `voiceprint_match_threshold`.
//! - [`wer`] — Word Error Rate (and CER): Levenshtein edit distance at the
//!   word or character level; the headline ASR accuracy metric.
//!
//! `secret_crypto` (private) holds the DPAPI round-trip that keeps API keys off
//! disk in the clear; it is an implementation detail of [`config`].

#![warn(missing_docs)]

pub mod backup;
pub mod catalog;
pub mod chunk;
pub mod config;
pub mod der;
pub mod diarization;
pub mod dictation;
pub mod doctor;
pub mod embed;
pub mod endpoints;
pub mod error;
pub mod export;
// Foreground-window detection for per-app dictation overrides. Exported on every
// platform — the module ships a non-Windows stub so the daemon can call it
// unconditionally and just get `None` off Windows.
pub mod foreground;
pub mod fusion;
pub mod hook;
pub mod id;
#[cfg(windows)]
pub mod job;
pub mod llm;
#[cfg(feature = "native-whisper")]
pub mod native_whisper;
pub mod profiles;
pub mod queue;
pub mod realign;
pub(crate) mod secret_crypto;
pub mod tags;
pub mod title;
pub mod transcription;
pub mod types;
pub mod voiceprint;
pub mod voiceprint_eval;
pub mod webhook;
pub mod wer;

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
    HookMetadata, HookPayload, ListFilter, ListKind, MeetingTrack, RecordMode, Recording,
    RecordingStatus, SpeakerName, TranscriptSegment,
};
