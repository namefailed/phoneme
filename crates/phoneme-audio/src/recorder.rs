//! Recorder — the public capture API.
//!
//! Wraps a [`Source`] with state management for start / stop / cancel /
//! auto-stop-on-silence / max-duration. Buffers samples in memory and writes
//! a WAV file on finalization.

use crate::format::AudioConfig;
use crate::silence::SilenceDetector;
use crate::source::Source;
use crate::wav;
use phoneme_core::error::{Error, Result};
use std::path::Path;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Same threshold as [`crate::meeting_align::QUIET_THRESHOLD`].
const FIRST_CONTENT_THRESHOLD: i16 = 100;

/// Minimum number of samples in a single block that must exceed
/// [`FIRST_CONTENT_THRESHOLD`] before the block counts as the first real
/// content. Guards `first_non_silent_at` against a lone spike / click (a
/// transient on the loopback device, a key press) being mistaken for the onset
/// of audio — which would relocate a sparse system track too early on the
/// shared timeline. ~32 samples is ~2 ms at 16 kHz.
const FIRST_CONTENT_MIN_LOUD_SAMPLES: usize = 32;

/// Whether `block` carries real content (a sustained run above threshold), used
/// to stamp `first_non_silent_at`. See [`FIRST_CONTENT_MIN_LOUD_SAMPLES`].
fn block_has_content(block: &[i16]) -> bool {
    block
        .iter()
        .filter(|&&s| s.saturating_abs() > FIRST_CONTENT_THRESHOLD)
        .count()
        >= FIRST_CONTENT_MIN_LOUD_SAMPLES
}

/// How the recorder should decide to stop.
///
/// This is [`phoneme_core::RecordMode`], re-exported under the audio crate's
/// historical name. There is one record-mode enum across the workspace (audit
/// A-H4) instead of three structurally-identical copies (core, this crate, and
/// the daemon). Variants: `Hold` (stop only on explicit `stop_and_finalize`),
/// `Oneshot` (auto-stop on silence), `Duration { secs }` (auto-stop after N s).
pub use phoneme_core::RecordMode as RecordingMode;

/// Stop-condition and silence-detection settings for a single recording.
///
/// [`Default`] gives Hold mode, a 5-minute cap, and a -45 dBFS / 3 s silence
/// gate — the values the daemon uses for an ordinary recording.
#[derive(Debug)]
pub struct RecorderConfig {
    /// How the recording decides to stop (hold until told, stop on silence, or
    /// stop after a fixed duration). See [`RecordingMode`].
    pub mode: RecordingMode,
    /// Hard ceiling on captured length, in milliseconds. The recording always
    /// stops here even in Hold mode, and the buffer is truncated to exactly this
    /// many samples so the cap is never overshot.
    pub max_duration_ms: u64,
    /// Silence gate for Oneshot auto-stop, in full-scale decibels (negative;
    /// quieter = more negative). Ignored in other modes. See [`SilenceDetector`].
    pub silence_threshold_dbfs: f32,
    /// How long the audio must stay below the gate before Oneshot stops, in
    /// milliseconds.
    pub silence_window_ms: u32,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            mode: RecordingMode::Hold,
            max_duration_ms: 300_000,
            silence_threshold_dbfs: -45.0,
            silence_window_ms: 3000,
        }
    }
}

/// Outcome of a recording that was finalized to a WAV file.
#[derive(Debug, Clone)]
pub struct RecordingResult {
    /// Length of the written audio in milliseconds, derived from the sample
    /// count at the canonical 16 kHz rate.
    pub duration_ms: i64,
    /// Number of `i16` samples written to the file (mono, so this equals the
    /// frame count).
    pub samples_written: usize,
}

/// Public recorder handle. Owns the background capture task.
pub struct Recorder {
    cfg: AudioConfig,
    cmd_tx: mpsc::Sender<RecorderCommand>,
    task: JoinHandle<Result<TaskOutput>>,
    /// Peak-normalization ceiling in dBFS, applied to the captured buffer right
    /// before the WAV is written. `None` (the default) leaves the audio at its
    /// captured level; `Some(target_dbfs)` boosts a quiet recording so its
    /// loudest sample lands on that ceiling. See [`Recorder::with_normalize`]
    /// and [`crate::normalize::normalize_peak`]. Only the *finalized* recording
    /// is normalized — the live preview is never touched.
    normalize_target_dbfs: Option<f32>,
}

enum RecorderCommand {
    Stop,
    Cancel,
    Pause,
    Resume,
    /// Reply with `(total_len, samples)` where `samples` is a clone of at most
    /// the last `max_tail` captured samples (or all of them when `max_tail == 0`
    /// or exceeds the buffer), and `total_len` is the full captured length so
    /// far. Capture is never disturbed. Used by the streaming preview to
    /// transcribe a bounded trailing window without copying the whole (growing)
    /// buffer every tick.
    Snapshot {
        max_tail: usize,
        reply: tokio::sync::oneshot::Sender<(usize, Vec<i16>)>,
    },
}

struct TaskOutput {
    samples: Vec<i16>,
    duration_ms: i64,
    cancelled: bool,
    /// Wall-clock instant when the first non-silent block was captured.
    first_non_silent_at: Option<Instant>,
}

/// A cheap, cloneable, read-only handle for snapshotting a live recorder's
/// captured audio without owning the [`Recorder`] itself.
///
/// It holds a clone of the recorder's command channel, so it can ask the capture
/// task for a trailing window exactly like [`Recorder::snapshot_tail`] does —
/// but it can be handed to a *separate* task (e.g. the daemon's streaming
/// preview loop) and outlive the borrow of the `Recorder`. This is what lets the
/// live preview read a single recording's audio AND a meeting mic-track's audio
/// through one uniform path: the daemon keeps the `Recorder` wherever it likes
/// (an `Arc<Mutex<…>>` for the single path, inside the `ActiveMeeting` for the
/// meeting path) and just clones out a `SnapshotHandle` for the preview.
///
/// `snapshot_tail` returns `Err` once the recorder task has ended (stop/cancel),
/// which the preview loop treats as "recording gone — exit".
#[derive(Clone)]
pub struct SnapshotHandle {
    cmd_tx: mpsc::Sender<RecorderCommand>,
}

impl SnapshotHandle {
    /// See [`Recorder::snapshot_tail`]. Returns `(total_len, tail_samples)`.
    pub async fn snapshot_tail(&self, max_tail: usize) -> Result<(usize, Vec<i16>)> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.cmd_tx
            .send(RecorderCommand::Snapshot {
                max_tail,
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("recorder task is gone".into()))?;
        rx.await
            .map_err(|_| Error::Internal("recorder dropped snapshot reply".into()))
    }
}

impl Recorder {
    /// Begin recording with the given source. The task starts pulling
    /// immediately.
    pub async fn start(
        source: Box<dyn Source>,
        cfg: RecorderConfig,
        on_done: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Result<Self> {
        Self::start_with_prepend(source, cfg, on_done, Vec::new()).await
    }

    /// Begin recording, seeding the output with `prepend` samples *before* live
    /// capture begins. Used for the pre-roll feature: the daemon hands over the
    /// last few hundred milliseconds of buffered microphone audio so the first
    /// syllable isn't clipped. The prepended samples are treated as already-
    /// captured audio — they are not fed to the silence detector (they're
    /// historical, not "now") but do count toward the max-duration cap.
    ///
    /// `prepend` must already be in the source's canonical format (16 kHz mono
    /// i16). An empty `prepend` is identical to [`Recorder::start`].
    pub async fn start_with_prepend(
        mut source: Box<dyn Source>,
        cfg: RecorderConfig,
        on_done: Option<tokio::sync::oneshot::Sender<()>>,
        prepend: Vec<i16>,
    ) -> Result<Self> {
        let audio_cfg = source.config();
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<RecorderCommand>(4);

        let task = tokio::spawn(async move {
            let mut samples: Vec<i16> = if prepend.is_empty() {
                Vec::with_capacity(audio_cfg.sample_rate.as_u32() as usize)
            } else {
                prepend
            };
            let mut detector = SilenceDetector::new(
                cfg.silence_threshold_dbfs,
                cfg.silence_window_ms,
                audio_cfg.sample_rate.as_u32(),
            );
            let max_samples =
                (cfg.max_duration_ms * audio_cfg.sample_rate.as_u32() as u64 / 1000) as usize;
            let duration_samples = match cfg.mode {
                RecordingMode::Duration { secs } => {
                    Some(secs as u64 * audio_cfg.sample_rate.as_u32() as u64)
                }
                _ => None,
            };

            let mut cancelled = false;
            let mut is_paused = false;
            let mut should_drain = false;
            let mut first_non_silent_at: Option<Instant> = None;
            // Set in the Pause/Resume arms; forwarded to the source after the
            // select! (can't borrow `source` mutably inside it — it's already
            // borrowed by `next_block`).
            let mut pending_pause: Option<bool> = None;

            loop {
                tokio::select! {
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(RecorderCommand::Stop) => { should_drain = true; break; }
                            Some(RecorderCommand::Cancel) => { cancelled = true; break; }
                            Some(RecorderCommand::Pause) => { is_paused = true; pending_pause = Some(true); }
                            Some(RecorderCommand::Resume) => { is_paused = false; detector.reset(); pending_pause = Some(false); }
                            Some(RecorderCommand::Snapshot { max_tail, reply }) => {
                                // Hand back the full captured length plus a clone
                                // of at most the last `max_tail` samples (all when
                                // 0). Cloning only the tail keeps the per-tick cost
                                // constant regardless of recording length. Capture
                                // continues uninterrupted; a dropped receiver fails
                                // the send harmlessly.
                                let total = samples.len();
                                let start = if max_tail == 0 || max_tail >= total {
                                    0
                                } else {
                                    total - max_tail
                                };
                                let _ = reply.send((total, samples[start..].to_vec()));
                            }
                            None => break,
                        }
                    }
                    block = source.next_block() => {
                        // A capture-device failure (e.g. the mic was unplugged)
                        // now surfaces as `Err` from `next_block` instead of
                        // hanging. The source drains its buffered audio into
                        // `samples` as `Ok(Some(_))` first and only then yields
                        // the error, so DON'T propagate it — that would discard
                        // the whole take. Log it and break, letting the loop fall
                        // through and finalize the audio captured up to the drop.
                        let block = match block {
                            Ok(b) => b,
                            Err(e) => {
                                tracing::warn!(error = %e, "audio source failed mid-capture; finalizing the partial recording");
                                break;
                            }
                        };
                        match block {
                            Some(b) => {
                                if !is_paused {
                                    if first_non_silent_at.is_none() && block_has_content(&b) {
                                        first_non_silent_at = Some(Instant::now());
                                    }
                                    // Silence detector is only used for Oneshot mode auto-stop.
                                    // In Hold mode (meeting mode), it's called but never triggers
                                    // a stop, so no audio is trimmed based on silence.
                                    detector.push(&b);
                                    samples.extend_from_slice(&b);
                                    if cfg.mode == RecordingMode::Oneshot && detector.is_silent() {
                                        break;
                                    }
                                    if let Some(target) = duration_samples {
                                        if samples.len() as u64 >= target {
                                            samples.truncate(target as usize);
                                            break;
                                        }
                                    }
                                    if samples.len() >= max_samples {
                                        samples.truncate(max_samples);
                                        break;
                                    }
                                }
                            }
                            None => break,
                        }
                    }
                }

                // Forward a pause/resume to the source *after* the select! so we
                // aren't holding the `next_block` borrow. Loopback uses resume to
                // re-baseline its gap-fill clock so a paused span isn't filled
                // with silence; dense sources (mic) no-op.
                if let Some(paused) = pending_pause.take() {
                    source.set_paused(paused).await;
                }
            }

            // On an explicit stop, drain audio the source already buffered plus
            // its flushed final partial chunk, so the trailing fraction of a
            // second isn't lost. Stop capture first so no post-stop audio is
            // recorded; the drain ends when the source closes its channel.
            if should_drain {
                let _ = source.stop().await;
                while let Ok(Some(b)) = source.next_block().await {
                    if !is_paused {
                        if first_non_silent_at.is_none() && block_has_content(&b) {
                            first_non_silent_at = Some(Instant::now());
                        }
                        samples.extend_from_slice(&b);
                    }
                }
            }

            let duration_ms =
                (samples.len() as u64 * 1000 / audio_cfg.sample_rate.as_u32() as u64) as i64;

            if let Some(tx) = on_done {
                let _ = tx.send(());
            }

            Ok(TaskOutput {
                samples,
                duration_ms,
                cancelled,
                first_non_silent_at,
            })
        });

        Ok(Self {
            cfg: audio_cfg,
            cmd_tx,
            task,
            normalize_target_dbfs: None,
        })
    }

    /// Enable peak normalization of the finalized recording.
    ///
    /// When set, the captured buffer is peak-normalized to `target_dbfs`
    /// full-scale decibels (a negative ceiling such as `-1.0`) just before the
    /// WAV is written by [`Self::stop_and_finalize`] or
    /// [`Self::wait_for_finalize`]. A quiet recording is boosted so its loudest
    /// sample lands on that ceiling, which gives transcription a healthier
    /// signal; an already-loud or silent recording is left untouched. See
    /// [`crate::normalize::normalize_peak`] for the exact guards.
    ///
    /// Normalization is off by default. This is a consuming builder so it can be
    /// chained right after [`Self::start`] without changing that call's
    /// signature; [`Self::set_normalize`] is the equivalent setter when the
    /// recorder is already owned mutably.
    pub fn with_normalize(mut self, target_dbfs: f32) -> Self {
        self.normalize_target_dbfs = Some(target_dbfs);
        self
    }

    /// Set or clear the finalize-time peak-normalization ceiling. `Some` enables
    /// it (see [`Self::with_normalize`]); `None` disables it. Affects only the
    /// WAV written on finalize, never the live preview.
    pub fn set_normalize(&mut self, target_dbfs: Option<f32>) {
        self.normalize_target_dbfs = target_dbfs;
    }

    /// The format this recorder captures into — always the source's canonical
    /// 16 kHz mono config, and the format the finalized WAV will carry.
    pub fn audio_config(&self) -> AudioConfig {
        self.cfg
    }

    /// Stop the recording (politely) and write its samples to `path`. The
    /// stored samples remain in memory until this call returns.
    pub async fn stop_and_finalize(self, path: &Path) -> Result<RecordingResult> {
        let _ = self.cmd_tx.send(RecorderCommand::Stop).await;
        let out = self
            .task
            .await
            .map_err(|e| Error::Internal(format!("recorder task: {e}")))??;
        if out.cancelled {
            return Err(Error::Internal("recording was cancelled".into()));
        }
        let mut samples = out.samples;
        if let Some(target_dbfs) = self.normalize_target_dbfs {
            crate::normalize::normalize_peak(&mut samples, target_dbfs);
        }
        wav::write_wav(path, &samples, self.cfg)?;
        Ok(RecordingResult {
            duration_ms: out.duration_ms,
            samples_written: samples.len(),
        })
    }

    /// Stop recording and return the raw samples without writing a WAV file.
    /// This is useful for post-processing (e.g., padding for meeting track synchronization).
    pub async fn stop_and_get_samples(self) -> Result<(Vec<i16>, i64, Option<Instant>)> {
        let _ = self.cmd_tx.send(RecorderCommand::Stop).await;
        let out = self
            .task
            .await
            .map_err(|e| Error::Internal(format!("recorder task: {e}")))??;
        if out.cancelled {
            return Err(Error::Internal("recording was cancelled".into()));
        }
        Ok((out.samples, out.duration_ms, out.first_non_silent_at))
    }

    /// Discard the recording. No WAV file is written.
    pub async fn cancel(self) -> Result<()> {
        let _ = self.cmd_tx.send(RecorderCommand::Cancel).await;
        let _ = self.task.await;
        Ok(())
    }

    /// Pause the recording. Audio frames will be pulled but discarded.
    pub async fn pause(&self) -> Result<()> {
        let _ = self.cmd_tx.send(RecorderCommand::Pause).await;
        Ok(())
    }

    /// Resume the recording after a pause.
    pub async fn resume(&self) -> Result<()> {
        let _ = self.cmd_tx.send(RecorderCommand::Resume).await;
        Ok(())
    }

    /// Clone the samples captured so far without disturbing capture. Used by the
    /// streaming-preview loop to transcribe the in-progress recording. The
    /// capture task answers from its in-memory buffer, so calling this does not
    /// pause, stop, or otherwise change the recording in any way.
    pub async fn snapshot(&self) -> Result<Vec<i16>> {
        // `max_tail = 0` => the full buffer.
        let (_total, samples) = self.snapshot_tail(0).await?;
        Ok(samples)
    }

    /// Like [`Self::snapshot`], but clones at most the last `max_tail` samples and also
    /// returns the full captured length so far as `(total_len, tail_samples)`.
    /// The streaming preview uses this to transcribe a bounded trailing window
    /// (constant per-tick cost) while still knowing how much total audio exists
    /// so it can throttle on newly-accumulated samples.
    pub async fn snapshot_tail(&self, max_tail: usize) -> Result<(usize, Vec<i16>)> {
        self.snapshot_handle().snapshot_tail(max_tail).await
    }

    /// A cheap, cloneable, read-only handle that can snapshot this recorder's
    /// captured audio from another task without owning the `Recorder`. Used by
    /// the daemon's live-preview loop so it can read either a single recording's
    /// or a meeting mic track's audio through one uniform path. The handle stays
    /// valid until the recorder is stopped/cancelled (after which `snapshot_tail`
    /// returns `Err`).
    pub fn snapshot_handle(&self) -> SnapshotHandle {
        SnapshotHandle {
            cmd_tx: self.cmd_tx.clone(),
        }
    }

    /// Wait for the recorder to auto-finalize (Oneshot / Duration modes,
    /// or the source returning `None`) and then write the WAV file.
    pub async fn wait_for_finalize(self, path: &Path) -> Result<RecordingResult> {
        let out = self
            .task
            .await
            .map_err(|e| Error::Internal(format!("recorder task: {e}")))??;
        if out.cancelled {
            return Err(Error::Internal("recording was cancelled".into()));
        }
        let mut samples = out.samples;
        if let Some(target_dbfs) = self.normalize_target_dbfs {
            crate::normalize::normalize_peak(&mut samples, target_dbfs);
        }
        wav::write_wav(path, &samples, self.cfg)?;
        Ok(RecordingResult {
            duration_ms: out.duration_ms,
            samples_written: samples.len(),
        })
    }
}
