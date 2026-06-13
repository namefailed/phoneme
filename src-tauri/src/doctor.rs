//! Doctor checks for the GUI.
//!
//! The implementation lives in [`phoneme_core::doctor`] so the GUI and the CLI
//! (`phoneme doctor`) share one check-result type and one set of probes (audit
//! A-H3). This module just re-exports it under the tray's existing path so the
//! `#[tauri::command]` wrappers in `commands.rs` keep referring to
//! `crate::doctor::*`.

pub use phoneme_core::doctor::{
    run_backend_checks_with_ports, run_local_checks, CheckResult, EffectiveWhisperPorts,
};
