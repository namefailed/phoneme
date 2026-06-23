//! `phoneme voice list|rename|forget|restore|merge …` — manage the
//! cross-recording named-voice library (the CLI face of the GUI Speaker Library
//! manager, #9). Distinct from `phoneme speaker`, which names the diarized
//! labels *within one recording*; this manages the library those names are
//! matched against.
//!
//! `list` is observe-only (`Client::connect_observe`, like `phoneme entities` /
//! `phoneme tag list`): inspecting the library shouldn't spawn a daemon. The
//! mutations (rename/forget/restore/merge) take the spawning `Client::connect`.
//! `forget` is reversible — `restore` undoes it. The bool-returning ops
//! (`forget`/`restore`/`merge`) report which way they went and exit non-zero
//! when nothing changed (unknown id), so scripts can detect a no-op.

use crate::args::{VoiceAction, VoiceArgs};
use crate::client::Client;
use crate::output;
use phoneme_core::{Config, NamedVoice};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: VoiceArgs, cfg: &Config, json: bool) -> ExitCode {
    match args.action {
        None | Some(VoiceAction::List) => list(cfg, json).await,
        Some(VoiceAction::Rename { id, name }) => {
            mutate(cfg, json, Request::RenameNamedVoice { id, name }, "renamed").await
        }
        Some(VoiceAction::Forget { id }) => {
            flag(
                cfg,
                json,
                Request::ForgetNamedVoice { id },
                "removed",
                "forgotten (undo with `phoneme voice restore <id>`)",
                "no such voice (or already forgotten)",
            )
            .await
        }
        Some(VoiceAction::Restore { id }) => {
            flag(
                cfg,
                json,
                Request::UndoForgetNamedVoice { id },
                "restored",
                "restored",
                "nothing to restore (unknown id, or not forgotten)",
            )
            .await
        }
        Some(VoiceAction::Merge { from_id, into_id }) => {
            flag(
                cfg,
                json,
                Request::MergeNamedVoices { from_id, into_id },
                "merged",
                "merged",
                "nothing merged (unknown voice id)",
            )
            .await
        }
    }
}

/// `phoneme voice list` — print the named-voice library (name · samples · id).
async fn list(cfg: &Config, json: bool) -> ExitCode {
    let mut client = match Client::connect_observe(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let value = match client.send(Request::ListNamedVoices).await {
        Ok(v) => v,
        Err(code) => return code,
    };
    let voices: Vec<NamedVoice> = match serde_json::from_value(value) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: parsing named-voice list: {e}");
            return ExitCode::FAILURE;
        }
    };

    if json {
        output::print_json_lines(&voices);
    } else if voices.is_empty() {
        println!("no named voices yet — name a recognized speaker in the app to enroll one");
    } else {
        for v in &voices {
            let s = if v.samples == 1 { "sample" } else { "samples" };
            println!("{}  ({} {})  [{}]", v.name, v.samples, s, v.id);
        }
    }
    ExitCode::SUCCESS
}

/// Send a mutation whose Ok payload we don't inspect; print `done` on success.
async fn mutate(cfg: &Config, json: bool, req: Request, done: &str) -> ExitCode {
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.send(req).await {
        Ok(_) => {
            if !json {
                println!("{done}");
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

/// Send a mutation whose Ok payload is `{<key>: bool}` (forget/restore/merge);
/// report `yes`/`no` and exit non-zero on a no-op so scripts can tell.
async fn flag(cfg: &Config, json: bool, req: Request, key: &str, yes: &str, no: &str) -> ExitCode {
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.send(req).await {
        Ok(v) => {
            let ok = v.get(key).and_then(|b| b.as_bool()).unwrap_or(false);
            if json {
                output::print_json(&v);
            } else {
                println!("{}", if ok { yes } else { no });
            }
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(code) => code,
    }
}
