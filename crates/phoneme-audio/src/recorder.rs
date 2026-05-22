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
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// How the recorder should decide to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingMode {
    /// Stop only when an external caller invokes `stop_and_finalize`.
    Hold,
    /// Auto-stop when silence is detected.
    Oneshot,
    /// Auto-stop after exactly N seconds.
    Duration { secs: u32 },
}

#[derive(Debug, Clone)]
pub struct RecorderConfig {
    pub mode: RecordingMode,
    pub max_duration_ms: u64,
    pub silence_threshold_dbfs: f32,
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

#[derive(Debug, Clone)]
pub struct RecordingResult {
    pub duration_ms: i64,
    pub samples_written: usize,
}

/// Public recorder handle. Owns the background capture task.
pub struct Recorder {
    cfg: AudioConfig,
    cmd_tx: mpsc::Sender<RecorderCommand>,
    task: JoinHandle<Result<TaskOutput>>,
}

enum RecorderCommand {
    Stop,
    Cancel,
}

struct TaskOutput {
    samples: Vec<i16>,
    duration_ms: i64,
    cancelled: bool,
}

impl Recorder {
    /// Begin recording with the given source. The task starts pulling
    /// immediately.
    pub async fn start(mut source: Box<dyn Source>, cfg: RecorderConfig, on_done: Option<tokio::sync::oneshot::Sender<()>>) -> Result<Self> {
        let audio_cfg = source.config();
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<RecorderCommand>(4);

        let task = tokio::spawn(async move {
            let mut samples: Vec<i16> = Vec::with_capacity(audio_cfg.sample_rate.as_u32() as usize);
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

            loop {
                tokio::select! {
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(RecorderCommand::Stop) => break,
                            Some(RecorderCommand::Cancel) => { cancelled = true; break; }
                            None => break,
                        }
                    }
                    block = source.next_block() => {
                        match block? {
                            Some(b) => {
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
                            None => break,
                        }
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
            })
        });

        Ok(Self {
            cfg: audio_cfg,
            cmd_tx,
            task,
        })
    }

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
        wav::write_wav(path, &out.samples, self.cfg)?;
        Ok(RecordingResult {
            duration_ms: out.duration_ms,
            samples_written: out.samples.len(),
        })
    }

    /// Discard the recording. No WAV file is written.
    pub async fn cancel(self) -> Result<()> {
        let _ = self.cmd_tx.send(RecorderCommand::Cancel).await;
        let _ = self.task.await;
        Ok(())
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
        wav::write_wav(path, &out.samples, self.cfg)?;
        Ok(RecordingResult {
            duration_ms: out.duration_ms,
            samples_written: out.samples.len(),
        })
    }
}
