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

use crate::app_state::AppState;
use phoneme_core::config::DiarizationConfig;
use phoneme_core::id::RecordingId;
use phoneme_core::types::RecordingStatus;
use phoneme_core::Error;
use phoneme_ipc::schema::{DaemonEvent, PipelineStage};
use std::path::PathBuf;

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

async fn run(state: &AppState, id: &RecordingId, audio_path: &PathBuf) -> Result<(), Error> {
    let cfg = state.config.load();
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Transcribing,
    });

    // Same gate the pipeline takes: for the local server this yields the live
    // preview; a dictation clip is short, so in practice this jumps straight
    // past the serial inbox queue the normal pipeline waits in.
    let permit = state.whisper_sem.acquire().await;
    // Diarization is never run for dictation — speaker labels in typed text
    // would be noise, and the model pass costs more than the transcription.
    let provider = state.transcription.provider(
        cfg.in_place_provider_config(),
        &DiarizationConfig::default(),
    );
    let language = cfg.whisper.language.clone().filter(|s| !s.is_empty());
    let transcription = provider
        .transcribe_with_segments(audio_path, language.as_deref())
        .await?;
    drop(permit);

    let raw = transcription.text.clone();
    let polished = match cfg.in_place.cleanup.as_str() {
        "off" => raw.clone(),
        // A full LLM round-trip through the configured post-processing
        // provider — the user explicitly chose polish over latency.
        "llm" => match state.llm.provider(&cfg.llm_post_process) {
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
        // persists below (when saving), and the error reaches the UI.
        tracing::error!(id = %id.as_str(), error = %e, "in-place dictation: failed to insert text");
        state.events.emit(DaemonEvent::TranscriptionFailed {
            id: id.clone(),
            error: format!("dictation transcribed but couldn't type at the cursor: {e}"),
        });
    }

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
