//! Abstraction over an audio sample source.
//!
//! Production code uses [`CpalSource`] which wraps a CPAL input stream;
//! tests use [`SyntheticSource`] which is hand-fed sample buffers.

use crate::convert::{downmix_to_mono_f32, f32_to_i16, i16_to_f32};
use crate::format::{AudioConfig, SampleRate};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat as CpalSampleFormat, StreamConfig};
use phoneme_core::error::{Error, Result};
use std::sync::Arc;
use tokio::sync::mpsc;

/// One block of i16 samples (already converted to Phoneme's canonical format:
/// 16-bit mono PCM at 16 kHz).
pub type SampleBlock = Vec<i16>;

/// An asynchronous source of audio sample blocks. Implementations must convert
/// to Phoneme's canonical format (16-bit, 16 kHz, mono) before yielding.
#[async_trait::async_trait]
pub trait Source: Send {
    /// Configuration that this source produces. Always reports the canonical
    /// format that downstream consumers will see (after any internal
    /// conversion).
    fn config(&self) -> AudioConfig;

    /// Pull the next block of samples. Returns `Ok(None)` when the source has
    /// been stopped and drained.
    async fn next_block(&mut self) -> Result<Option<SampleBlock>>;

    /// Stop the underlying capture. After calling, `next_block` should return
    /// `Ok(None)` shortly.
    async fn stop(&mut self) -> Result<()>;
}

/// A synthetic source: backed by an mpsc channel that tests push samples into.
///
/// Closing the sender side causes `next_block` to return `None`.
pub struct SyntheticSource {
    cfg: AudioConfig,
    rx: mpsc::Receiver<SampleBlock>,
}

impl SyntheticSource {
    pub fn new(cfg: AudioConfig) -> (Self, SyntheticSink) {
        let (tx, rx) = mpsc::channel(64);
        (Self { cfg, rx }, SyntheticSink { tx })
    }
}

/// Companion handle that tests use to push samples and then close.
#[derive(Clone)]
pub struct SyntheticSink {
    tx: mpsc::Sender<SampleBlock>,
}

impl SyntheticSink {
    pub async fn push(&self, block: SampleBlock) -> Result<()> {
        self.tx
            .send(block)
            .await
            .map_err(|_| Error::Internal("synthetic sink dropped".into()))
    }

    /// Close the sink, causing the matched source to return `None`.
    pub fn close(self) {
        drop(self.tx);
    }
}

#[async_trait::async_trait]
impl Source for SyntheticSource {
    fn config(&self) -> AudioConfig {
        self.cfg
    }

    async fn next_block(&mut self) -> Result<Option<SampleBlock>> {
        Ok(self.rx.recv().await)
    }

    async fn stop(&mut self) -> Result<()> {
        self.rx.close();
        Ok(())
    }
}

/// Cancellation handle held by callers; dropping it tells the CPAL source to
/// stop the underlying stream.
pub type StopHandle = Arc<tokio::sync::Notify>;

/// A CPAL-backed source that converts the device's native format to
/// 16-bit mono 16 kHz before yielding sample blocks.
///
/// Architecture:
///
/// 1. The CPAL callback (real-time OS thread) does the minimum possible work —
///    pushes a fresh `Vec<f32>` of raw samples into a bounded mpsc channel via
///    `try_send`. If the channel is full (consumer fell behind), the block is
///    dropped — preferable to blocking the audio thread.
/// 2. A background tokio worker task drains the channel, performs downmix +
///    resample + format conversion off the real-time thread, and forwards
///    canonical `Vec<i16>` blocks downstream.
/// 3. The `cpal::Stream` itself is `!Send` on Windows (WASAPI's COM apartment
///    requires the stream to live on the thread that built it), so we own it
///    inside a dedicated `std::thread`. The stream-thread blocks until a stop
///    signal arrives, then drops the stream — which stops capture cleanly.
pub struct CpalSource {
    cfg: AudioConfig,
    out_rx: mpsc::Receiver<SampleBlock>,
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
    _stream_thread: Option<std::thread::JoinHandle<()>>,
    _worker: tokio::task::JoinHandle<()>,
}

impl CpalSource {
    pub fn open(device: cpal::Device) -> Result<Self> {
        let supported = device
            .default_input_config()
            .map_err(|e| Error::Internal(format!("cpal default_input_config: {e}")))?;

        let device_sample_rate = supported.sample_rate().0;
        let device_channels = supported.channels() as usize;
        let device_format = supported.sample_format();
        let stream_cfg: StreamConfig = supported.into();

        // Raw samples (f32, interleaved at device format) → worker.
        let (raw_tx, mut raw_rx) = mpsc::channel::<Vec<f32>>(64);
        // Canonical samples (i16 mono 16 kHz) → downstream consumer.
        let (out_tx, out_rx) = mpsc::channel::<SampleBlock>(64);
        // Stop signal to the stream-owning thread.
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        // One-shot ready signal: the stream thread reports build/play success
        // or failure before we return Self.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();

        // The cpal::Stream is `!Send` on Windows (COM thread affinity), so
        // it lives entirely on a dedicated OS thread.
        let stream_thread = std::thread::Builder::new()
            .name("phoneme-cpal-stream".into())
            .spawn(move || {
                // CPAL callback: minimal real-time-safe work — copy + try_send.
                let stream_result = match device_format {
                    CpalSampleFormat::F32 => {
                        let raw_tx = raw_tx.clone();
                        device.build_input_stream(
                            &stream_cfg,
                            move |data: &[f32], _| {
                                // Allocate-and-copy is the only "work" on the
                                // audio thread. No downmix, no resample, no
                                // synchronous send.
                                let _ = raw_tx.try_send(data.to_vec());
                            },
                            |err| tracing::warn!("cpal stream error: {err}"),
                            None,
                        )
                    }
                    CpalSampleFormat::I16 => {
                        let raw_tx = raw_tx.clone();
                        device.build_input_stream(
                            &stream_cfg,
                            move |data: &[i16], _| {
                                // Cheap i16→f32 normalize is fine on the audio
                                // thread (one float multiply per sample).
                                let f32s = i16_to_f32(data);
                                let _ = raw_tx.try_send(f32s);
                            },
                            |err| tracing::warn!("cpal stream error: {err}"),
                            None,
                        )
                    }
                    other => {
                        let _ = ready_tx.send(Err(Error::Internal(format!(
                            "unsupported CPAL sample format: {other:?}"
                        ))));
                        return;
                    }
                };

                let stream = match stream_result {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(Error::Internal(format!(
                            "cpal build_input_stream: {e}"
                        ))));
                        return;
                    }
                };

                if let Err(e) = stream.play() {
                    let _ = ready_tx.send(Err(Error::Internal(format!("cpal play: {e}"))));
                    return;
                }

                // Drop the original `raw_tx` clone — only the callback's owned
                // copy keeps the sender side alive. Once the stream is dropped,
                // the callback goes with it and `raw_rx` closes.
                drop(raw_tx);

                let _ = ready_tx.send(Ok(()));

                // Block until stop is requested, then drop the stream.
                let _ = stop_rx.recv();
                drop(stream);
            })
            .map_err(|e| Error::Internal(format!("spawn cpal stream thread: {e}")))?;

        // Wait for the stream thread to either start successfully or fail.
        ready_rx
            .recv()
            .map_err(|_| Error::Internal("cpal stream thread died before reporting".into()))??;

        // Background worker — does all the heavy lifting off the audio thread.
        let target_rate = SampleRate::HZ_16K.as_u32();
        let worker = tokio::spawn(async move {
            use rubato::{
                Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType,
                WindowFunction,
            };
            let chunk_frames = 4096;

            let mut resampler = if device_sample_rate != target_rate {
                let params = SincInterpolationParameters {
                    sinc_len: 128,
                    f_cutoff: 0.95,
                    interpolation: SincInterpolationType::Linear,
                    oversampling_factor: 128,
                    window: WindowFunction::BlackmanHarris2,
                };
                let ratio = target_rate as f64 / device_sample_rate as f64;
                SincFixedIn::<f32>::new(ratio, 1.0, params, chunk_frames, 1).ok()
            } else {
                None
            };

            let mut accumulator = Vec::new();

            while let Some(raw) = raw_rx.recv().await {
                accumulator.extend_from_slice(&raw);

                while accumulator.len() >= chunk_frames * device_channels {
                    let chunk: Vec<f32> = accumulator
                        .drain(..chunk_frames * device_channels)
                        .collect();
                    let mono = downmix_to_mono_f32(&chunk, device_channels);

                    let processed = if let Some(r) = resampler.as_mut() {
                        match r.process(&[mono], None) {
                            Ok(out) => out.into_iter().next().unwrap_or_default(),
                            Err(e) => {
                                tracing::warn!("resample failed: {e}");
                                continue;
                            }
                        }
                    } else {
                        mono
                    };

                    let i16s = f32_to_i16(&processed);
                    if out_tx.send(i16s).await.is_err() {
                        return;
                    }
                }
            }

            // Flush the final partial chunk (zero-padded to one full chunk) so
            // the trailing fraction of a second isn't dropped when capture stops.
            if !accumulator.is_empty() {
                let needed = chunk_frames * device_channels;
                accumulator.resize(needed, 0.0);
                let mono = downmix_to_mono_f32(&accumulator, device_channels);
                let processed = if let Some(r) = resampler.as_mut() {
                    r.process(&[mono], None)
                        .ok()
                        .and_then(|out| out.into_iter().next())
                        .unwrap_or_default()
                } else {
                    mono
                };
                let _ = out_tx.send(f32_to_i16(&processed)).await;
            }
        });

        Ok(Self {
            cfg: AudioConfig::phoneme_default(),
            out_rx,
            stop_tx: Some(stop_tx),
            _stream_thread: Some(stream_thread),
            _worker: worker,
        })
    }
}

#[async_trait::async_trait]
impl Source for CpalSource {
    fn config(&self) -> AudioConfig {
        self.cfg
    }

    async fn next_block(&mut self) -> Result<Option<SampleBlock>> {
        Ok(self.out_rx.recv().await)
    }

    async fn stop(&mut self) -> Result<()> {
        // Tell the stream thread to drop its cpal::Stream, which stops capture.
        // Do NOT close out_rx here: let the worker drain its accumulator and
        // flush the final partial chunk first. The channel closes naturally when
        // the worker finishes (out_tx is dropped), which ends the recorder drain.
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        Ok(())
    }
}

impl Drop for CpalSource {
    fn drop(&mut self) {
        // Signal stop on drop so the stream thread exits even if `stop()` was
        // never called explicitly.
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn synthetic_yields_blocks_then_none_on_close() {
        let cfg = AudioConfig::phoneme_default();
        let (mut src, sink) = SyntheticSource::new(cfg);
        sink.push(vec![1, 2, 3]).await.unwrap();
        sink.push(vec![4, 5, 6]).await.unwrap();
        sink.close();
        assert_eq!(src.next_block().await.unwrap(), Some(vec![1, 2, 3]));
        assert_eq!(src.next_block().await.unwrap(), Some(vec![4, 5, 6]));
        assert_eq!(src.next_block().await.unwrap(), None);
    }

    #[tokio::test]
    async fn synthetic_stop_drains_then_returns_none() {
        let cfg = AudioConfig::phoneme_default();
        let (mut src, _sink) = SyntheticSource::new(cfg);
        src.stop().await.unwrap();
        assert_eq!(src.next_block().await.unwrap(), None);
    }
}
