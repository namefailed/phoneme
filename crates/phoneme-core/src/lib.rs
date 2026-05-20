//! phoneme-core — shared library for the Phoneme voice notes app.
//!
//! This crate is platform-agnostic and provides the building blocks consumed
//! by `phoneme-daemon`, `phoneme` (CLI), and the Tauri tray app:
//!
//! - configuration loading
//! - the error taxonomy used over IPC
//! - the SQLite recordings catalog
//! - the filesystem-backed inbox queue
//! - the HTTP transcription client
//! - the subprocess hook runner
//!
//! Modules will be added in subsequent tasks.
