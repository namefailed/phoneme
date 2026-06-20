//! Abstraction over an audio sample source.
//!
//! Production code uses [`CpalSource`] which wraps a CPAL input stream;
//! tests use [`SyntheticSource`] which is hand-fed sample buffers.

use crate::convert::{downmix_to_mono_f32, f32_to_i16};
use crate::format::{AudioConfig, SampleRate};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat as CpalSampleFormat, StreamConfig};
use phoneme_core::config::CaptureSource;
use phoneme_core::error::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// One block of i16 samples (already converted to Phoneme's canonical format:
/// 16-bit mono PCM at 16 kHz).
pub type SampleBlock = Vec<i16>;

/// Number of silence samples to insert to keep a gap-filled (loopback) track
/// continuous: how many canonical samples *should* exist by now (`expected`)
/// minus how many are already accounted for (`delivered`) — but only once that
/// difference exceeds `tolerance`, so normal scheduling jitter between capture
/// callbacks isn't mistaken for a real silent gap. Saturating, so a
/// momentarily-ahead `delivered` (timing skew) yields 0 rather than
/// underflowing.
///
/// `delivered` must be the audio clock — samples the device actually handed to
/// the capture callback (plus fill silence already inserted) — NOT the count
/// this worker has emitted downstream. Under CPU load the worker drains late,
/// and measuring its own emissions counts the backlog still sitting in the
/// channel/accumulator as missing: a scheduler stall then reads as a "gap",
/// silence is over-inserted, and the late real audio lands on top of it,
/// running the track long. Samples the device truly delivered can never be
/// declared a gap, no matter how late they are processed.
fn gap_fill_len(expected: usize, delivered: usize, tolerance: usize) -> usize {
    match expected.checked_sub(delivered) {
        Some(gap) if gap > tolerance => gap,
        _ => 0,
    }
}

/// Number of canonical-rate (`target_rate`) samples equivalent to `frames`
/// frames captured at `device_rate`. Rounds down; the sub-sample remainder is
/// orders of magnitude below the gap-fill tolerance, so the truncation can
/// neither fabricate nor hide a gap. Returns 0 for a (pathological) zero
/// device rate instead of dividing by zero.
fn frames_to_canonical(frames: usize, device_rate: u32, target_rate: u32) -> usize {
    if device_rate == 0 {
        return 0;
    }
    ((frames as u128 * target_rate as u128) / device_rate as u128) as usize
}

/// Convert one native device sample to `f32` in `[-1.0, 1.0]` using cpal's
/// dasp-backed lossless conversions: signed types scale by their full range,
/// unsigned types shift their origin (e.g. u16's 32768) to 0.0 first. Split out
/// of the capture callback so the per-format mapping is unit-testable without
/// audio hardware.
fn sample_to_f32<T>(s: T) -> f32
where
    T: cpal::Sample,
    f32: cpal::FromSample<T>,
{
    s.to_sample::<f32>()
}

/// Build the CPAL input stream for native sample type `T`, doing the minimum
/// real-time-safe work in the callback: count the samples the device delivered
/// (the audio clock the gap filler trusts — see [`gap_fill_len`]), convert each
/// to `f32` (one linear map per sample), and hand the block to the worker
/// without ever blocking the audio thread.
///
/// `device_lost` / `stop_signal` give the (real-time) error callback a way out:
/// when CPAL reports a stream error — a mic unplugged mid-recording is the
/// motivating case — it flags `device_lost` so the source can surface the
/// failure, and pokes `stop_signal` so the stream-owning thread unblocks from
/// `stop_rx.recv()` and drops the stream. Without that, the error callback only
/// logged: the stream thread blocked forever, `raw_tx` never dropped, the
/// worker's `raw_rx.recv()` never returned `None`, and capture hung silently
/// until the recorder's max-duration cap fired.
fn build_input_stream_as_f32<T>(
    device: &cpal::Device,
    stream_cfg: &StreamConfig,
    pool_rx: std::sync::mpsc::Receiver<Vec<f32>>,
    raw_tx: mpsc::Sender<Vec<f32>>,
    delivered: Arc<std::sync::atomic::AtomicUsize>,
    device_lost: Arc<AtomicBool>,
    stop_signal: std::sync::mpsc::Sender<()>,
) -> std::result::Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    device.build_input_stream(
        stream_cfg,
        move |data: &[T], _| {
            // The audio clock: interleaved samples the DEVICE delivered,
            // counted before any queueing/processing delay can distort them.
            delivered.fetch_add(data.len(), std::sync::atomic::Ordering::Relaxed);
            // Use a pre-allocated buffer if available, else allocate.
            let mut buf = pool_rx
                .try_recv()
                .unwrap_or_else(|_| Vec::with_capacity(data.len()));
            buf.clear();
            buf.extend(data.iter().map(|&s| sample_to_f32(s)));
            let _ = raw_tx.try_send(buf);
        },
        move |err| {
            tracing::warn!("cpal stream error: {err}");
            // Surface the failure and tear the stream down. A device error
            // (e.g. the mic was unplugged) won't recover on its own; flag it so
            // the source can report it, and signal the stream thread to drop the
            // stream so the capture pipeline drains and `next_block` returns.
            device_lost.store(true, Ordering::Relaxed);
            let _ = stop_signal.send(());
        },
        None,
    )
}

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

    /// Notify the source that recording was paused (`true`) or resumed
    /// (`false`). Used by gap-filling sources (loopback) to avoid back-filling a
    /// paused span as silence. Default: no-op (dense sources need nothing).
    async fn set_paused(&mut self, _paused: bool) {}
}

/// A synthetic source: backed by an mpsc channel that tests push samples into.
///
/// Closing the sender side causes `next_block` to return `None`.
pub struct SyntheticSource {
    cfg: AudioConfig,
    rx: mpsc::Receiver<SampleBlock>,
}

impl SyntheticSource {
    /// Create a source paired with the sink that feeds it. The source reports
    /// `cfg` as its format and yields exactly the blocks pushed into the sink, in
    /// order, until the sink is closed or dropped.
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
    /// Enqueue one block for the paired source to yield. Awaits if the channel
    /// is full. Returns `Err` once the source side has been dropped.
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

/// A self-generating source that produces constant-value sample blocks without
/// any external controller or real audio hardware.
///
/// Unlike [`SyntheticSource`] (which is sink-fed by tests), `GeneratorSource`
/// needs no external controller — it produces blocks of i16 samples and sleeps
/// for the corresponding wall-clock duration between blocks to mimic a live
/// hardware source. Used by the daemon when `PHONEME_AUDIO_BACKEND=synthetic`
/// is set (CI / headless tests) so the recorder lifecycle can be exercised
/// without real audio hardware.
pub struct GeneratorSource {
    cfg: AudioConfig,
    block_frames: usize,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl GeneratorSource {
    /// Create a source that yields `block_frames` frames per call.
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
    /// Set true on resume so the gap-fill worker re-baselines its wall clock and
    /// doesn't back-fill a paused span as silence. Only meaningful when the
    /// source was opened with gap filling (loopback).
    rebaseline_fill_clock: Arc<std::sync::atomic::AtomicBool>,
    /// Flipped true by the CPAL error callback when the device fails mid-capture
    /// (e.g. the mic is unplugged). Checked once the stream drains so the drop
    /// is reported as an error rather than a clean end-of-stream.
    device_lost: Arc<AtomicBool>,
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

    /// Like [`Self::open`], but tears the stream down `tail_grace` after a stop is
    /// requested so the audio still sitting in the OS capture buffer at stop
    /// time is delivered instead of being discarded (avoids clipping the final
    /// fraction of a second). Use `Duration::ZERO` for sources where the tail
    /// is irrelevant (e.g. the rolling pre-roll buffer).
    pub fn open_with_grace(device: cpal::Device, tail_grace: Duration) -> Result<Self> {
        let supported = device
            .default_input_config()
            .map_err(|e| Error::Internal(format!("cpal default_input_config: {e}")))?;
        // Microphone capture is continuous (the device always delivers), so it
        // never needs wall-clock gap filling.
        Self::open_with_config(device, supported, tail_grace, false)
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

    /// Like [`Self::open_kind`], but applies a `tail_grace` teardown delay (see
    /// [`Self::open_with_grace`]) so a manually-stopped recording keeps the audio that
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

    /// [`Self::system_audio`] with a `tail_grace` teardown delay (see
    /// [`Self::open_with_grace`]).
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
        // WASAPI loopback delivers samples ONLY while something is playing — it
        // hands back nothing during silence (e.g. a paused video), which would
        // collapse silent gaps and desync the meeting's two tracks. Enable
        // wall-clock gap filling so the loopback track stays continuous.
        Self::open_with_config(device, supported, tail_grace, true)
    }

    fn open_with_config(
        device: cpal::Device,
        supported: cpal::SupportedStreamConfig,
        tail_grace: Duration,
        fill_gaps: bool,
    ) -> Result<Self> {
        let device_sample_rate = supported.sample_rate().0;
        let device_channels = supported.channels() as usize;
        let device_format = supported.sample_format();
        let stream_cfg: StreamConfig = supported.into();

        // Raw samples (f32, interleaved at device format) → worker.
        let (raw_tx, mut raw_rx) = mpsc::channel::<Vec<f32>>(64);
        // Pre-allocated buffers to prevent audio thread allocations.
        let (pool_tx, pool_rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(64);
        for _ in 0..64 {
            let _ = pool_tx.try_send(Vec::with_capacity(4096));
        }
        // Canonical samples (i16 mono 16 kHz) → downstream consumer.
        let (out_tx, out_rx) = mpsc::channel::<SampleBlock>(64);
        // Stop signal to the stream-owning thread.
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        // One-shot ready signal: the stream thread reports build/play success
        // or failure before we return Self.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();

        // The audio clock: interleaved samples the device has delivered to the
        // capture callback so far. The gap-fill worker compares THIS (not its
        // own emission count) against the wall-clock expectation, so worker
        // scheduling lag under CPU load can never read as a silent gap — see
        // `gap_fill_len`.
        let delivered_raw = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let delivered_cb = delivered_raw.clone();

        // Set by the CPAL error callback when the device drops mid-capture; lets
        // the source report the failure once the pipeline drains. The callback
        // also pokes a `stop_tx` clone so the stream thread unblocks and tears
        // the stream down (see `build_input_stream_as_f32`).
        let device_lost = Arc::new(AtomicBool::new(false));
        let device_lost_cb = device_lost.clone();
        let stop_tx_cb = stop_tx.clone();

        // The cpal::Stream is `!Send` on Windows (COM thread affinity), so
        // it lives entirely on a dedicated OS thread.
        let stream_thread = std::thread::Builder::new()
            .name("phoneme-cpal-stream".into())
            .spawn(move || {
                // CPAL callback: minimal real-time-safe work — count + convert
                // + try_send, shared across formats by the generic builder.
                // Every format cpal 0.15 can report is covered; the enum is
                // `#[non_exhaustive]`, so a format added by a future cpal must
                // land in the error arm rather than capturing garbage.
                let stream_result = match device_format {
                    CpalSampleFormat::F32 => build_input_stream_as_f32::<f32>(
                        &device,
                        &stream_cfg,
                        pool_rx,
                        raw_tx.clone(),
                        delivered_cb,
                        device_lost_cb,
                        stop_tx_cb,
                    ),
                    CpalSampleFormat::F64 => build_input_stream_as_f32::<f64>(
                        &device,
                        &stream_cfg,
                        pool_rx,
                        raw_tx.clone(),
                        delivered_cb,
                        device_lost_cb,
                        stop_tx_cb,
                    ),
                    CpalSampleFormat::I8 => build_input_stream_as_f32::<i8>(
                        &device,
                        &stream_cfg,
                        pool_rx,
                        raw_tx.clone(),
                        delivered_cb,
                        device_lost_cb,
                        stop_tx_cb,
                    ),
                    CpalSampleFormat::I16 => build_input_stream_as_f32::<i16>(
                        &device,
                        &stream_cfg,
                        pool_rx,
                        raw_tx.clone(),
                        delivered_cb,
                        device_lost_cb,
                        stop_tx_cb,
                    ),
                    CpalSampleFormat::I32 => build_input_stream_as_f32::<i32>(
                        &device,
                        &stream_cfg,
                        pool_rx,
                        raw_tx.clone(),
                        delivered_cb,
                        device_lost_cb,
                        stop_tx_cb,
                    ),
                    CpalSampleFormat::I64 => build_input_stream_as_f32::<i64>(
                        &device,
                        &stream_cfg,
                        pool_rx,
                        raw_tx.clone(),
                        delivered_cb,
                        device_lost_cb,
                        stop_tx_cb,
                    ),
                    CpalSampleFormat::U8 => build_input_stream_as_f32::<u8>(
                        &device,
                        &stream_cfg,
                        pool_rx,
                        raw_tx.clone(),
                        delivered_cb,
                        device_lost_cb,
                        stop_tx_cb,
                    ),
                    CpalSampleFormat::U16 => build_input_stream_as_f32::<u16>(
                        &device,
                        &stream_cfg,
                        pool_rx,
                        raw_tx.clone(),
                        delivered_cb,
                        device_lost_cb,
                        stop_tx_cb,
                    ),
                    CpalSampleFormat::U32 => build_input_stream_as_f32::<u32>(
                        &device,
                        &stream_cfg,
                        pool_rx,
                        raw_tx.clone(),
                        delivered_cb,
                        device_lost_cb,
                        stop_tx_cb,
                    ),
                    CpalSampleFormat::U64 => build_input_stream_as_f32::<u64>(
                        &device,
                        &stream_cfg,
                        pool_rx,
                        raw_tx.clone(),
                        delivered_cb,
                        device_lost_cb,
                        stop_tx_cb,
                    ),
                    other => {
                        let device_name =
                            device.name().unwrap_or_else(|_| "<unnamed device>".into());
                        let _ = ready_tx.send(Err(Error::Internal(format!(
                            "unsupported sample format {other:?} from input device \
                             \"{device_name}\" — supported: f32/f64/i8/i16/i32/i64/u8/u16/u32/u64"
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
        // Flipped true by `set_paused(false)` (resume) so the fill worker forgets
        // wall-clock that elapsed during a pause instead of back-filling it as
        // silence (which would desync a meeting's tracks across a pause).
        let rebaseline_fill_clock = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let rebaseline_worker = rebaseline_fill_clock.clone();
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
            // Max size: 10 seconds of raw float audio per channel. If we hit this, the consumer is dead.
            let max_accumulator_len = device_sample_rate as usize * device_channels * 10;

            // Wall-clock gap filling (loopback only — see `fill_gaps`). A gap
            // is declared only when the wall clock says more samples should
            // exist than the device has actually DELIVERED to the capture
            // callback (`delivered_raw`, the audio clock) plus the silence
            // already inserted (`filled`). When a block arrives after the
            // device went quiet, that shortfall is real silence and we insert
            // it first so the track stays continuous and time-aligned. Blocks
            // still queued behind a lagging worker are already counted in
            // `delivered_raw`, so scheduler stalls under CPU load can never
            // fake a gap (they used to, when this compared against the
            // worker's own emission count).
            let mut fill_start = std::time::Instant::now();
            // Canonical samples of fill silence inserted so far. Counted
            // separately from device deliveries so an already-filled gap is
            // never re-detected and filled again.
            let mut filled: usize = 0;
            // 100 ms at 16 kHz — only fill gaps larger than this so ordinary
            // scheduling jitter between callbacks is never mistaken for silence.
            let fill_tolerance = target_rate as usize / 10;
            // Cap a single fill-silence allocation at 1 s of canonical samples.
            // A very long silent gap (e.g. an overnight loopback recording with
            // nothing playing) would otherwise allocate one enormous `vec![0; gap]`
            // — hundreds of MB — and append it downstream in one shot. Emitting the
            // gap in bounded blocks keeps memory flat and lets the recorder apply
            // its max-duration cap between blocks instead of after a giant append.
            let fill_block_max = target_rate as usize;

            while let Some(mut raw) = raw_rx.recv().await {
                if fill_gaps {
                    // Device deliveries so far, in the canonical timebase.
                    let delivered_frames = delivered_raw.load(std::sync::atomic::Ordering::Relaxed)
                        / device_channels.max(1);
                    let delivered =
                        frames_to_canonical(delivered_frames, device_sample_rate, target_rate)
                            + filled;
                    // On resume, re-baseline so wall-clock that elapsed during a
                    // (meeting) pause is forgotten — otherwise we'd insert the
                    // whole paused span as silence here and desync the tracks.
                    // After this, `expected == delivered`, so no gap is filled
                    // for the pause; the post-resume timeline starts fresh in
                    // step with the mic track (which resumed at the same instant).
                    if rebaseline_worker.swap(false, std::sync::atomic::Ordering::Relaxed) {
                        fill_start = std::time::Instant::now()
                            - std::time::Duration::from_secs_f64(
                                delivered as f64 / target_rate as f64,
                            );
                    }
                    let expected =
                        (fill_start.elapsed().as_secs_f64() * target_rate as f64) as usize;
                    let gap = gap_fill_len(expected, delivered, fill_tolerance);
                    if gap > 0 {
                        // Emit the gap as bounded blocks (<=1 s each) rather than
                        // one allocation, so a long silent stretch can't OOM.
                        let mut remaining = gap;
                        while remaining > 0 {
                            let block = remaining.min(fill_block_max);
                            if out_tx.send(vec![0i16; block]).await.is_err() {
                                return;
                            }
                            remaining -= block;
                        }
                        filled += gap;
                    }
                }

                if accumulator.len() + raw.len() > max_accumulator_len {
                    tracing::warn!(
                        "audio accumulator overflow ({} frames), dropping buffered audio!",
                        accumulator.len()
                    );
                    accumulator.clear();
                }
                accumulator.extend_from_slice(&raw);

                // Recycle buffer
                raw.clear();
                let _ = pool_tx.try_send(raw);

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
            rebaseline_fill_clock,
            device_lost,
        })
    }
}

#[async_trait::async_trait]
impl Source for CpalSource {
    fn config(&self) -> AudioConfig {
        self.cfg
    }

    async fn next_block(&mut self) -> Result<Option<SampleBlock>> {
        match self.out_rx.recv().await {
            Some(block) => Ok(Some(block)),
            // Channel drained. If the device failed mid-capture (the error
            // callback flagged it), surface that as an error so the recorder
            // doesn't mistake a hardware disconnect for a clean stop. Otherwise
            // it's a normal end-of-stream.
            None if self.device_lost.load(Ordering::Relaxed) => Err(Error::Internal(
                "audio capture device failed mid-recording (e.g. disconnected)".into(),
            )),
            None => Ok(None),
        }
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

    async fn set_paused(&mut self, paused: bool) {
        // On resume, tell the fill worker to re-baseline its wall clock so the
        // paused span isn't back-filled with silence (which would lengthen the
        // loopback track past the mic and desync the meeting). Nothing to do on
        // pause: the recorder discards captured audio while paused, and any fill
        // silence emitted meanwhile is discarded with it.
        if !paused {
            self.rebaseline_fill_clock
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
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

    #[test]
    fn gap_fill_only_beyond_tolerance() {
        let tol = 1600; // 100 ms at 16 kHz
                        // No elapsed, nothing delivered → nothing to fill.
        assert_eq!(gap_fill_len(0, 0, tol), 0);
        // Small gap within tolerance (jitter) → no fill.
        assert_eq!(gap_fill_len(1000, 0, tol), 0);
        // A 5-second pause (80 000 samples) → fill all of it.
        assert_eq!(gap_fill_len(80_000, 0, tol), 80_000);
        // Mid-stream gap: expected far ahead of delivered → fill the difference.
        assert_eq!(gap_fill_len(100_000, 20_000, tol), 80_000);
        // Delivered momentarily ahead of expected (timing skew) → saturate to 0.
        assert_eq!(gap_fill_len(500, 1000, tol), 0);
    }

    #[test]
    fn gap_fill_ignores_worker_lag() {
        // The worker-lag scenario: 5 s of wall clock elapsed (expected 80 000
        // canonical samples) and the device HAS delivered all 5 s — the worker
        // just hasn't processed the queued blocks yet (CPU load). With the gap
        // math based on the audio clock (delivered), no gap exists; the old
        // emission-based accounting saw the backlog as missing and inserted
        // silence the late blocks then landed on top of.
        let tol = 1600;
        let delivered = frames_to_canonical(240_000, 48_000, 16_000); // 5 s at 48 kHz
        assert_eq!(delivered, 80_000);
        assert_eq!(gap_fill_len(80_000, delivered, tol), 0);
    }

    #[test]
    fn gap_fill_detects_true_device_silence() {
        // The device went quiet after 2 s while the wall clock ran to 5 s: the
        // missing 3 s are real loopback silence and must be filled in full —
        // the audio-clock rebase must not stop true gaps from being detected.
        let tol = 1600;
        let delivered = frames_to_canonical(96_000, 48_000, 16_000); // 2 s at 48 kHz
        assert_eq!(delivered, 32_000);
        assert_eq!(gap_fill_len(80_000, delivered, tol), 48_000);
    }

    #[test]
    fn gap_fill_counts_inserted_silence_once() {
        // After a 3 s gap is filled, the fill itself counts toward `delivered`
        // (the worker adds `filled`), so the same gap is never inserted twice.
        let tol = 1600;
        let delivered_plus_filled = 32_000 + 48_000;
        assert_eq!(gap_fill_len(80_000, delivered_plus_filled, tol), 0);
    }

    #[test]
    fn frames_to_canonical_converts_device_rates() {
        // Typical loopback (48 kHz) and mic (44.1 kHz) rates down to 16 kHz.
        assert_eq!(frames_to_canonical(48_000, 48_000, 16_000), 16_000);
        assert_eq!(frames_to_canonical(44_100, 44_100, 16_000), 16_000);
        // Already canonical → identity.
        assert_eq!(frames_to_canonical(16_000, 16_000, 16_000), 16_000);
        // Nothing delivered → nothing expected.
        assert_eq!(frames_to_canonical(0, 48_000, 16_000), 0);
        // Pathological zero device rate must not divide by zero.
        assert_eq!(frames_to_canonical(1000, 0, 16_000), 0);
    }

    #[test]
    fn sample_conversion_maps_each_format_to_f32_range() {
        // Pin the conversion contract for every device format the capture
        // path accepts: the type's origin maps to 0.0 (the unsigned formats'
        // mid-range origin — e.g. u16's 32768 — is the classic trap), the
        // minimum to -1.0, and the maximum to just under +1.0.
        assert_eq!(sample_to_f32(0.5f32), 0.5);
        assert_eq!(sample_to_f32(0.25f64), 0.25);

        assert_eq!(sample_to_f32(0i8), 0.0);
        assert_eq!(sample_to_f32(i8::MIN), -1.0);
        assert!(sample_to_f32(i8::MAX) > 0.98);

        assert_eq!(sample_to_f32(0i16), 0.0);
        assert_eq!(sample_to_f32(i16::MIN), -1.0);
        assert!(sample_to_f32(i16::MAX) > 0.9999);

        assert_eq!(sample_to_f32(0i32), 0.0);
        assert_eq!(sample_to_f32(i32::MIN), -1.0);

        assert_eq!(sample_to_f32(0i64), 0.0);
        assert_eq!(sample_to_f32(i64::MIN), -1.0);

        assert_eq!(sample_to_f32(1u8 << 7), 0.0);
        assert_eq!(sample_to_f32(0u8), -1.0);
        assert!(sample_to_f32(u8::MAX) > 0.98);

        assert_eq!(sample_to_f32(1u16 << 15), 0.0);
        assert_eq!(sample_to_f32(0u16), -1.0);
        assert!(sample_to_f32(u16::MAX) > 0.9999);

        assert_eq!(sample_to_f32(1u32 << 31), 0.0);
        assert_eq!(sample_to_f32(0u32), -1.0);

        assert_eq!(sample_to_f32(1u64 << 63), 0.0);
        assert_eq!(sample_to_f32(0u64), -1.0);
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
