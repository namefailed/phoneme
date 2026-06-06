//! Abstraction over an audio sample source.
//!
//! Production code uses [`CpalSource`] which wraps a CPAL input stream;
//! tests use [`SyntheticSource`] which is hand-fed sample buffers.

use crate::convert::{downmix_to_mono_f32, f32_to_i16, i16_to_f32};
use crate::format::{AudioConfig, SampleRate};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat as CpalSampleFormat, StreamConfig};
use phoneme_core::config::CaptureSource;
use phoneme_core::error::{Error, Result};
use std::sync::Arc;
use std::time::Duration;
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

/// A self-driving synthetic source that generates silence at real-time pace.
///
/// Unlike [`SyntheticSource`] (which is sink-fed by tests), `GeneratorSource`
/// needs no external controller — it produces blocks of i16 zeros and sleeps
/// for the corresponding wall-clock duration between blocks to mimic a live
/// hardware source.  Used by the daemon when `PHONEME_AUDIO_BACKEND=synthetic`
/// is set (CI / headless tests) so the recorder lifecycle can be exercised
/// without real audio hardware.
pub struct GeneratorSource {
    cfg: AudioConfig,
    block_frames: usize,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl GeneratorSource {
    /// Create a source that yields `block_frames` frames of silence per call.
    ///
    /// `block_frames = 1600` gives 100 ms blocks at 16 kHz — enough resolution
    /// for the recorder to respond to stop() promptly without spinning.
    pub fn new(block_frames: usize) -> Self {
        Self {
            cfg: AudioConfig::phoneme_default(),
            block_frames,
            stopped: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

#[async_trait::async_trait]
impl Source for GeneratorSource {
    fn config(&self) -> AudioConfig {
        self.cfg
    }

    async fn next_block(&mut self) -> Result<Option<SampleBlock>> {
        if self.stopped.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(None);
        }
        // Sleep for the real-time duration of one block so the recorder's drain
        // loop terminates at a sane pace and the WAV output has realistic length.
        let block_dur = std::time::Duration::from_secs_f64(
            self.block_frames as f64 / self.cfg.sample_rate.as_u32() as f64,
        );
        tokio::time::sleep(block_dur).await;
        if self.stopped.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(None);
        }
        Ok(Some(vec![1000i16; self.block_frames]))
    }

    async fn stop(&mut self) -> Result<()> {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
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
    /// Open the given device as a microphone (default input) capture.
    ///
    /// This is the historical entry point and keeps the original behavior: it
    /// uses the device's default *input* config and builds a normal input
    /// stream.
    pub fn open(device: cpal::Device) -> Result<Self> {
        Self::open_with_grace(device, Duration::ZERO)
    }

    /// Like [`open`], but tears the stream down `tail_grace` after a stop is
    /// requested so the audio still sitting in the OS capture buffer at stop
    /// time is delivered instead of being discarded (avoids clipping the final
    /// fraction of a second). Use `Duration::ZERO` for sources where the tail
    /// is irrelevant (e.g. the rolling pre-roll buffer).
    pub fn open_with_grace(device: cpal::Device, tail_grace: Duration) -> Result<Self> {
        let supported = device
            .default_input_config()
            .map_err(|e| Error::Internal(format!("cpal default_input_config: {e}")))?;
        Self::open_with_config(device, supported, tail_grace)
    }

    /// Open a capture source for the requested [`CaptureSource`].
    ///
    /// - [`CaptureSource::Microphone`] captures `device` as an input stream
    ///   (identical to [`CpalSource::open`]).
    /// - [`CaptureSource::SystemAudio`] ignores `device` and captures the
    ///   default *output* device in WASAPI loopback mode.
    pub fn open_kind(device: cpal::Device, kind: CaptureSource) -> Result<Self> {
        Self::open_kind_with_grace(device, kind, Duration::ZERO)
    }

    /// Like [`open_kind`], but applies a `tail_grace` teardown delay (see
    /// [`open_with_grace`]) so a manually-stopped recording keeps the audio that
    /// was still buffered in the OS at stop time.
    pub fn open_kind_with_grace(
        device: cpal::Device,
        kind: CaptureSource,
        tail_grace: Duration,
    ) -> Result<Self> {
        match kind {
            CaptureSource::Microphone => Self::open_with_grace(device, tail_grace),
            CaptureSource::SystemAudio => Self::system_audio_with_grace(tail_grace),
        }
    }

    /// Open a system-audio (loopback) capture on the default output device.
    ///
    /// On Windows, building an *input* stream on the default *output* (render)
    /// device makes cpal/WASAPI transparently capture loopback — i.e. whatever
    /// is currently playing through the speakers. The device's output format
    /// (typically 48 kHz stereo f32) feeds the same downmix → resample → i16
    /// pipeline used for the microphone path.
    ///
    /// Returns a clear error on platforms / hosts without a usable default
    /// output device or loopback support.
    pub fn system_audio() -> Result<Self> {
        Self::system_audio_with_grace(Duration::ZERO)
    }

    /// [`system_audio`] with a `tail_grace` teardown delay (see
    /// [`open_with_grace`]).
    pub fn system_audio_with_grace(tail_grace: Duration) -> Result<Self> {
        if !cfg!(windows) {
            return Err(Error::Internal(
                "system-audio capture (WASAPI loopback) is only available on Windows in this build"
                    .into(),
            ));
        }
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| {
            Error::Internal(
                "system-audio capture not available: no default output device for loopback".into(),
            )
        })?;
        // For loopback, the stream config must come from the output device's
        // default *output* config — the render endpoint's mix format.
        let supported = device
            .default_output_config()
            .map_err(|e| Error::Internal(format!("cpal default_output_config (loopback): {e}")))?;
        Self::open_with_config(device, supported, tail_grace)
    }

    fn open_with_config(
        device: cpal::Device,
        supported: cpal::SupportedStreamConfig,
        tail_grace: Duration,
    ) -> Result<Self> {
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

                // Block until stop is requested. Then, before tearing the
                // stream down, wait out `tail_grace` so CPAL/WASAPI can deliver
                // the frames it had already captured into the OS buffer at stop
                // time — otherwise that trailing fraction of a second is dropped
                // when the stream is destroyed (the recording sounds clipped at
                // the end). Zero for sources that don't care about the tail.
                let _ = stop_rx.recv();
                if !tail_grace.is_zero() {
                    std::thread::sleep(tail_grace);
                }
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

    #[test]
    fn capture_source_defaults_to_microphone() {
        assert_eq!(CaptureSource::default(), CaptureSource::Microphone);
    }

    #[cfg(not(windows))]
    #[test]
    fn system_audio_unavailable_off_windows() {
        // Real loopback can't be unit-tested with hardware, but the
        // platform-gate must report a clear, non-panicking error off Windows.
        let err = CpalSource::system_audio()
            .err()
            .expect("system_audio should error off Windows");
        assert!(format!("{err}").contains("only available on Windows"));
    }
}
