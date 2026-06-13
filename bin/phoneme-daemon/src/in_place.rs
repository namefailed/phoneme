//! The dictation fast lane: in-place recordings skip the inbox queue and the
//! full pipeline entirely (unless `[in_place].full_pipeline` opts back in).
//!
//! Flow: transcribe with the dictation provider → polish (rule-based by
//! default, zero latency) → type/paste at the cursor → only THEN persist to
//! the library in the background. A dictation never waits behind a meeting
//! that's mid-transcription, never runs diarization, and never pays an LLM
//! round-trip unless `cleanup = "llm"`.
//!
//! The recorder spawns [`spawn_fast_lane`] from `stop()` instead of enqueuing
//! the recording. Stage events still fire (Transcribing → Done/Failed), so
//! the queue panel, status column, and step notifications all track a
//! dictation exactly like a queued recording.
//!
//! With `full_pipeline` + `type_first` there is a second, type-only variant
//! ([`spawn_type_first`]): the same transcribe → polish → type core runs for
//! the instant typing, but the recording itself rides the normal queue — the
//! pipeline owns every catalog write, status, and event, and skips its own
//! end-of-run typing so the text never lands twice.

use crate::app_state::AppState;
use phoneme_core::config::DiarizationConfig;
use phoneme_core::id::RecordingId;
use phoneme_core::transcription::{DiarizationTrack, Transcription};
use phoneme_core::types::RecordingStatus;
use phoneme_core::Error;
use phoneme_ipc::schema::{DaemonEvent, PipelineStage};
use std::path::{Path, PathBuf};

/// Run the fast lane for a just-stopped in-place recording. Detached: errors
/// surface through the catalog status + `TranscriptionFailed` (toasted by the
/// UI), never a panic.
pub fn spawn_fast_lane(state: AppState, id: RecordingId, audio_path: PathBuf) {
    tokio::spawn(async move {
        if let Err(e) = run(&state, &id, &audio_path).await {
            tracing::error!(id = %id.as_str(), error = %e, "in-place fast lane failed");
            let _ = state
                .catalog
                .update_status(&id, RecordingStatus::TranscribeFailed)
                .await;
            state.events.emit(DaemonEvent::PipelineStageChanged {
                id: id.clone(),
                stage: PipelineStage::Failed,
            });
            state.events.emit(DaemonEvent::TranscriptionFailed {
                id,
                error: e.to_string(),
            });
        }
    });
}

/// Run the type-only pass for a just-stopped in-place recording when
/// `[in_place].full_pipeline` AND `[in_place].type_first` are on: type the
/// quick transcription at the cursor now, while the queued pipeline — the
/// recorder enqueued the recording alongside spawning this — does everything
/// else in the background. The pipeline owns ALL of the recording's state:
/// catalog writes, segments, statuses, stage events, the inbox item, and the
/// library copy. This task touches none of it, and the pipeline skips its own
/// end-of-run typing (see `pipeline_should_type`) so the text lands exactly
/// once.
///
/// Detached. A failure here costs only the instant typing — and since the
/// pipeline won't type either in this mode, the toast tells the user their
/// words are still coming to the library, just not to the cursor.
pub fn spawn_type_first(state: AppState, id: RecordingId, audio_path: PathBuf) {
    tokio::spawn(async move {
        if let Err(e) = transcribe_polish_type(&state, &id, &audio_path).await {
            tracing::error!(id = %id.as_str(), error = %e, "in-place type-first pass failed");
            // No status flip, no Failed stage: the recording is fine — it's
            // still queued and the pipeline retries transcription itself.
            state.events.emit(DaemonEvent::TranscriptionFailed {
                id,
                error: format!(
                    "dictation couldn't type your text right away ({e}) — the recording is still processing and the transcript will be in the library"
                ),
            });
        }
    });
}

async fn run(state: &AppState, id: &RecordingId, audio_path: &PathBuf) -> Result<(), Error> {
    let cfg = state.config.load();
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Transcribing,
    });

    let (transcription, polished) = transcribe_polish_type(state, id, audio_path).await?;
    let raw = transcription.text.clone();

    if cfg.in_place.save_to_library {
        // Persist AFTER the text has landed — the user already has their
        // words; none of this is on the latency path.
        state
            .catalog
            .update_transcript(id, &polished, &raw, "in-place")
            .await?;
        if let Err(e) = state
            .catalog
            .replace_segments(id, &transcription.segments)
            .await
        {
            tracing::warn!(id = %id.as_str(), "failed to persist dictation segments: {e}");
        }
        state
            .catalog
            .update_status(id, RecordingStatus::Done)
            .await?;
        state.events.emit(DaemonEvent::TranscriptionDone {
            id: id.clone(),
            transcript: polished.clone(),
        });
        let embedder_guard = state.embedder.read().await;
        if let Some(embedder) = embedder_guard.as_ref() {
            crate::pipeline::embed_and_store(embedder, &state.catalog, id, &polished).await;
        }
    } else {
        // Ephemeral mode: the typed text IS the product — drop the row + WAV.
        let _ = tokio::fs::remove_file(audio_path).await;
        state.catalog.delete(id).await?;
        state
            .events
            .emit(DaemonEvent::RecordingDeleted { id: id.clone() });
    }

    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Done,
    });
    Ok(())
}

/// The transcribe → polish → type core shared by both dictation variants:
/// the fast lane ([`run`], which persists afterwards) and the type-only pass
/// ([`spawn_type_first`], which does nothing else). Returns the raw
/// transcription (the fast lane persists its segments) and the polished text
/// that was typed.
///
/// A typing failure is deliberately NOT an `Err`: the words exist, so it is
/// logged and toasted (`TranscriptionFailed`) while the caller proceeds — the
/// fast lane still persists the transcript, and the type-first pass leaves
/// the queued pipeline to deliver it to the library.
async fn transcribe_polish_type(
    state: &AppState,
    id: &RecordingId,
    audio_path: &Path,
) -> Result<(Transcription, String), Error> {
    let cfg = state.config.load();

    // Same gate the pipeline takes: for the local server this yields the live
    // preview; a dictation clip is short, so in practice this jumps straight
    // past the serial inbox queue the normal pipeline waits in.
    let permit = state.whisper_sem.acquire().await;
    // Diarization is never run for dictation — speaker labels in typed text
    // would be noise, and the model pass costs more than the transcription.
    // Dictation's STT pick may point at the main or the preview bundled
    // server; `apply` follows either to the port it actually listens on.
    let stt_cfg = state
        .whisper_ports
        .apply(&cfg, cfg.in_place_provider_config());
    let provider = state
        .transcription
        .provider(&stt_cfg, &DiarizationConfig::default());
    let language = cfg.whisper.language.clone().filter(|s| !s.is_empty());
    let transcription = provider
        // Dictation never diarizes (the provider above already disables it) and
        // is never a meeting track, so the normal `Diarize` hint applies.
        .transcribe_with_segments(audio_path, language.as_deref(), DiarizationTrack::Diarize)
        .await?;
    drop(permit);

    let raw = transcription.text.clone();
    let polished = match cfg.in_place.cleanup.as_str() {
        "off" => raw.clone(),
        // A full LLM round-trip through the configured post-processing
        // provider — the user explicitly chose polish over latency.
        // `llm_provider_for_run` also launches the local Ollama when the
        // connection needs it (same as every queued LLM stage).
        "llm" => match crate::pipeline::llm_provider_for_run(state, &cfg.llm_post_process).await {
            Some(llm) => match llm.process(&cfg.llm_post_process.prompt, &raw).await {
                Ok(out) => out,
                Err(e) => {
                    tracing::warn!(error = %e, "in-place llm cleanup failed; typing raw text");
                    phoneme_core::dictation::fast_polish(&raw)
                }
            },
            None => phoneme_core::dictation::fast_polish(&raw),
        },
        // "fast" and anything unrecognized: the zero-latency rule polish.
        _ => phoneme_core::dictation::fast_polish(&raw),
    };

    if polished.trim().is_empty() {
        tracing::info!(id = %id.as_str(), "in-place dictation: nothing to type (empty transcript)");
    } else if let Err(e) = type_at_cursor(&polished, &cfg.in_place.type_mode).await {
        // Typing failing must not lose the words — the transcript still
        // persists (fast lane) or rides the queued pipeline into the library
        // (type-first), and the error reaches the UI.
        tracing::error!(id = %id.as_str(), error = %e, "in-place dictation: failed to insert text");
        state.events.emit(DaemonEvent::TranscriptionFailed {
            id: id.clone(),
            error: format!("dictation transcribed but couldn't type at the cursor: {e}"),
        });
    }

    Ok((transcription, polished))
}

/// Insert `text` at the system cursor. `mode` `"paste"` goes via the
/// clipboard (set → Ctrl+V → restore the previous clipboard) — near-instant
/// for long text; anything else types simulated keystrokes (works in apps
/// that block paste). Blocking input APIs run on a blocking thread.
pub(crate) async fn type_at_cursor(text: &str, mode: &str) -> Result<(), String> {
    let text = text.to_string();
    let paste = mode == "paste";
    tokio::task::spawn_blocking(move || {
        if paste {
            paste_blocking(&text)
        } else {
            type_blocking(&text)
        }
    })
    .await
    .map_err(|e| format!("input task panicked: {e}"))?
}

fn type_blocking(text: &str) -> Result<(), String> {
    use enigo::Keyboard;
    let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
        .map_err(|e| format!("input simulator init failed: {e}"))?;
    enigo.text(text).map_err(|e| format!("typing failed: {e}"))
}

fn paste_blocking(text: &str) -> Result<(), String> {
    use enigo::{Direction, Key, Keyboard};
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    // Best-effort restore — a non-text clipboard (image) simply isn't put back.
    let previous = clipboard.get_text().ok();
    clipboard
        .set_text(text)
        .map_err(|e| format!("clipboard write failed: {e}"))?;

    let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
        .map_err(|e| format!("input simulator init failed: {e}"))?;
    enigo
        .key(Key::Control, Direction::Press)
        .and_then(|_| enigo.key(Key::Unicode('v'), Direction::Click))
        .and_then(|_| enigo.key(Key::Control, Direction::Release))
        .map_err(|e| format!("paste keystroke failed: {e}"))?;

    // Give the target app time to consume the clipboard before restoring it —
    // Ctrl+V is processed asynchronously by the receiving window.
    std::thread::sleep(std::time::Duration::from_millis(150));
    if let Some(prev) = previous {
        let _ = clipboard.set_text(prev);
    }
    Ok(())
}
