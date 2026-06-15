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

/// Prepended to the in-place LLM cleanup prompt so spoken editing commands are
/// interpreted, not echoed. Kept local to dictation (not baked into the global
/// post-processing prompt). The rule-based `fast_polish` fallback applies the
/// same three commands, so behavior is consistent whether the LLM runs or not.
const VOICE_COMMAND_DIRECTIVES: &str = "The text is dictation that may contain spoken editing commands. \
Treat \"new line\" as a line break, \"new paragraph\" as a blank line, and \"scratch that\" (or \"delete that\") \
as an instruction to remove the immediately preceding phrase. Apply these edits and do not include the command words in the output.";

/// Run the fast lane for a just-stopped in-place recording. Detached: errors
/// surface through the catalog status + `TranscriptionFailed` (toasted by the
/// UI), never a panic.
pub fn spawn_fast_lane(
    state: AppState,
    id: RecordingId,
    audio_path: PathBuf,
    focused_app: Option<String>,
    focused_window_title: Option<String>,
) {
    tokio::spawn(async move {
        if let Err(e) = run(&state, &id, &audio_path, focused_app, focused_window_title).await {
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
pub fn spawn_type_first(
    state: AppState,
    id: RecordingId,
    audio_path: PathBuf,
    focused_app: Option<String>,
    focused_window_title: Option<String>,
) {
    tokio::spawn(async move {
        if let Err(e) =
            transcribe_polish_type(&state, &id, &audio_path, focused_app, focused_window_title).await
        {
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

/// A short, single-line title from the dictation text — the first few words,
/// capped to a sane length, trailing punctuation trimmed, with an ellipsis when
/// truncated. Empty when the text is blank. Pure + LLM-free so it works on any
/// box; the recordings list and detail header then show it like a normal title.
fn dictation_title_snippet(text: &str) -> String {
    const MAX_WORDS: usize = 8;
    const MAX_CHARS: usize = 60;
    let first_line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let all_words: Vec<&str> = first_line.split_whitespace().collect();
    let mut s: String = all_words.iter().take(MAX_WORDS).copied().collect::<Vec<_>>().join(" ");
    let mut truncated = all_words.len() > MAX_WORDS;
    if s.chars().count() > MAX_CHARS {
        s = s.chars().take(MAX_CHARS).collect();
        if let Some(idx) = s.rfind(' ') {
            s.truncate(idx);
        }
        truncated = true;
    }
    let s = s
        .trim_end_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
        .to_string();
    if s.is_empty() {
        String::new()
    } else if truncated {
        format!("{s}…")
    } else {
        s
    }
}

async fn run(
    state: &AppState,
    id: &RecordingId,
    audio_path: &PathBuf,
    focused_app: Option<String>,
    focused_window_title: Option<String>,
) -> Result<(), Error> {
    let cfg = state.config.load();
    state.events.emit(DaemonEvent::PipelineStageChanged {
        id: id.clone(),
        stage: PipelineStage::Transcribing,
    });

    let (transcription, polished) =
        transcribe_polish_type(state, id, audio_path, focused_app, focused_window_title).await?;
    let raw = transcription.text.clone();

    if cfg.in_place.save_to_library {
        // Persist AFTER the text has landed — the user already has their
        // words; none of this is on the latency path. Store the REAL model that
        // produced the text (same derivation as the pipeline) so the Transcript
        // model column reads like every other recording; the dictation marker is
        // the persisted `in_place` flag (shown as a badge in the detail pane),
        // not a fake model name.
        let model_label = cfg.in_place_provider_config().model_label();
        state
            .catalog
            .update_transcript(id, &polished, &raw, &model_label)
            .await?;
        if let Err(e) = state
            .catalog
            .replace_segments(id, &transcription.segments)
            .await
        {
            tracing::warn!(id = %id.as_str(), "failed to persist dictation segments: {e}");
        }
        // The fast lane skips the pipeline (and its LLM auto-title), so without a
        // title a dictation would fall back to showing the bare date as its title
        // — which hides the date from the detail meta line. Give it a cheap
        // content snippet as the title (no LLM, so it's reliable even when the
        // box can't run one); it reads like every other recording (title + date +
        // duration), and `is_auto = true` lets a later auto-title or the user
        // override it.
        let snippet = dictation_title_snippet(&polished);
        if !snippet.is_empty() {
            if let Err(e) = state.catalog.set_title(id, Some(&snippet), true, None).await {
                tracing::warn!(id = %id.as_str(), "failed to set dictation snippet title: {e}");
            }
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
    focused_app: Option<String>,
    focused_window_title: Option<String>,
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
        // Even with cleanup off, honor the spoken editing commands (the rule
        // pass is a no-op on dictation that doesn't contain them).
        "off" => phoneme_core::dictation::apply_voice_commands(&raw),
        // A full LLM round-trip through the configured post-processing
        // provider — the user explicitly chose polish over latency.
        // `llm_provider_for_run` also launches the local Ollama when the
        // connection needs it (same as every queued LLM stage). The dictation
        // voice-command directives are prepended so the LLM interprets
        // "new line"/"new paragraph"/"scratch that" rather than echoing them;
        // on failure we fall back to `fast_polish`, which applies the same
        // commands rule-based — consistent either way.
        "llm" => match crate::pipeline::llm_provider_for_run(state, &cfg.llm_post_process).await {
            Some(llm) => {
                let mut prompt =
                    format!("{VOICE_COMMAND_DIRECTIVES}\n\n{}", cfg.llm_post_process.prompt);
                // (6c) Opt-in app-aware context: when enabled (and the app was
                // not denylisted at capture time), prepend the focused window's
                // title so the LLM can adapt its polish to what you're working
                // in (code-ish in an editor, prose in a doc). The title is only
                // ever present here when `app_context` is on — it is never logged
                // and never goes anywhere but this cleanup prompt.
                if cfg.in_place.app_context {
                    if let Some(title) = focused_window_title.as_deref().filter(|t| !t.is_empty()) {
                        prompt = format!("Context — the active window is titled: {title}\n\n{prompt}");
                    }
                }
                match llm.process(&prompt, &raw).await {
                    Ok(out) => out,
                    Err(e) => {
                        tracing::warn!(error = %e, "in-place llm cleanup failed; typing raw text");
                        phoneme_core::dictation::fast_polish(&raw)
                    }
                }
            }
            None => phoneme_core::dictation::fast_polish(&raw),
        },
        // "fast" and anything unrecognized: the zero-latency rule polish
        // (which now includes the voice-command pass).
        _ => phoneme_core::dictation::fast_polish(&raw),
    };

    // (6b) Resolve how the text lands for the focused app: a per-app override
    // ("type"/"paste"/"off") wins over the global `type_mode`; an unlisted (or
    // undetectable) app falls back to the global mode. With the default empty
    // map this is always the global mode — today's behavior unchanged.
    let type_mode = cfg.in_place.resolve_type_mode(focused_app.as_deref());

    if polished.trim().is_empty() {
        tracing::info!(id = %id.as_str(), "in-place dictation: nothing to type (empty transcript)");
    } else if type_mode == "off" {
        // The user asked dictation NOT to auto-deliver for this app — the
        // transcript still persists (fast lane) or rides the pipeline into the
        // library, it just doesn't land at the cursor.
        tracing::info!(
            id = %id.as_str(),
            "in-place dictation: per-app override is \"off\" for the focused app; not typing"
        );
    } else if let Err(e) = type_at_cursor(&polished, type_mode).await {
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

#[cfg(test)]
mod tests {
    use super::dictation_title_snippet;
    use phoneme_core::config::InPlaceConfig;

    /// The exact resolution the dictation typing path relies on (6b): a per-app
    /// override decides type/paste/off for the focused app; an unlisted app —
    /// and the default empty map — fall back to the global `type_mode`.
    #[test]
    fn per_app_override_drives_the_dictation_type_mode() {
        let mut ip = InPlaceConfig::default();
        assert_eq!(ip.type_mode, "type");
        // Default: every app (and no focused app) types — today's behavior.
        assert_eq!(ip.resolve_type_mode(Some("code")), "type");
        assert_eq!(ip.resolve_type_mode(None), "type");

        ip.app_overrides.insert("code".into(), "paste".into());
        ip.app_overrides.insert("keepassxc".into(), "off".into());
        assert_eq!(ip.resolve_type_mode(Some("code")), "paste");
        assert_eq!(ip.resolve_type_mode(Some("keepassxc")), "off");
        assert_eq!(ip.resolve_type_mode(Some("notepad")), "type");
    }

    #[test]
    fn short_text_is_used_verbatim() {
        assert_eq!(dictation_title_snippet("buy milk and eggs"), "buy milk and eggs");
    }

    #[test]
    fn long_text_is_truncated_to_words_with_ellipsis() {
        let s = dictation_title_snippet("one two three four five six seven eight nine ten");
        assert_eq!(s, "one two three four five six seven eight…");
    }

    #[test]
    fn uses_the_first_nonblank_line_only() {
        assert_eq!(dictation_title_snippet("\n  \nhello there\nsecond line"), "hello there");
    }

    #[test]
    fn trailing_punctuation_is_trimmed_when_not_truncated() {
        assert_eq!(dictation_title_snippet("note to self."), "note to self");
    }

    #[test]
    fn blank_text_yields_empty() {
        assert_eq!(dictation_title_snippet("   \n  "), "");
    }
}
