use phoneme_audio::format::{AudioConfig, Channels, SampleRate};
use phoneme_audio::recorder::{RecorderConfig, RecordingMode};
use phoneme_audio::source::{SampleBlock, Source, SyntheticSink, SyntheticSource};
use phoneme_audio::wav;
use phoneme_audio::Recorder;
use phoneme_core::error::{Error, Result};
use std::time::Duration;
use tempfile::TempDir;

/// A source that yields a fixed number of blocks and then reports a capture
/// failure — the same shape `CpalSource::next_block` takes when the device's
/// error callback flags a disconnect (drain the buffered audio as `Ok(Some(_))`
/// first, then yield `Err`). Models a mic unplugged mid-recording so the
/// device-lost path can be exercised without real hardware.
struct FailingSource {
    cfg: AudioConfig,
    blocks: std::collections::VecDeque<SampleBlock>,
}

impl FailingSource {
    fn new(blocks: Vec<SampleBlock>) -> Self {
        Self {
            cfg: AudioConfig::phoneme_default(),
            blocks: blocks.into(),
        }
    }
}

#[async_trait::async_trait]
impl Source for FailingSource {
    fn config(&self) -> AudioConfig {
        self.cfg
    }

    async fn next_block(&mut self) -> Result<Option<SampleBlock>> {
        match self.blocks.pop_front() {
            Some(b) => Ok(Some(b)),
            // Buffered audio drained — now surface the device failure, just like
            // `CpalSource` does once the error callback has fired.
            None => Err(Error::Internal(
                "audio capture device failed mid-recording (e.g. disconnected)".into(),
            )),
        }
    }

    async fn stop(&mut self) -> Result<()> {
        // A device-lost teardown never reaches stop() in the loop, but a Stop
        // command would: drop any remaining blocks so the drain ends promptly.
        self.blocks.clear();
        Ok(())
    }
}

fn loud_block(samples: usize) -> Vec<i16> {
    (0..samples)
        .map(|i| ((i as f32 * 0.05).sin() * 20_000.0) as i16)
        .collect()
}

fn silent_block(samples: usize) -> Vec<i16> {
    vec![0; samples]
}

/// A deliberately quiet sine: peaks around 1000 of 32767 (~-30 dBFS), well above
/// the noise-floor guard but far below any sensible normalization ceiling.
fn quiet_block(samples: usize) -> Vec<i16> {
    (0..samples)
        .map(|i| ((i as f32 * 0.05).sin() * 1000.0) as i16)
        .collect()
}

/// Largest sample magnitude in a buffer.
fn peak_of(samples: &[i16]) -> i16 {
    samples
        .iter()
        .map(|&s| s.saturating_abs())
        .max()
        .unwrap_or(0)
}

fn make_synthetic() -> (SyntheticSource, SyntheticSink) {
    SyntheticSource::new(AudioConfig::phoneme_default())
}

#[tokio::test]
async fn hold_mode_writes_wav_with_pushed_samples() {
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("rec.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 10_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 1000,
    };
    let recorder = Recorder::start(Box::new(source), cfg, None).await.unwrap();

    sink.push(loud_block(8000)).await.unwrap(); // 0.5s
    sink.push(loud_block(8000)).await.unwrap(); // 1.0s total
    sink.close(); // signal end-of-stream — recorder will drain & exit naturally

    // Use `wait_for_finalize`, which awaits natural task completion via source
    // exhaustion, rather than `stop_and_finalize`. The latter races the
    // cmd-channel Stop against the source channel inside the recorder's unbiased
    // `tokio::select!`. That race is fine in production (CPAL never closes, so
    // Stop is the only exit), but here it makes the duration assertion flaky.
    let result = recorder.wait_for_finalize(&wav_path).await.unwrap();
    assert!(
        (result.duration_ms - 1000).abs() < 50,
        "duration was {}ms",
        result.duration_ms
    );
    let (samples, _) = wav::read_wav(&wav_path).unwrap();
    assert!(samples.len() >= 16_000 - 100); // ~1s worth of samples
}

#[tokio::test]
async fn cancel_does_not_write_wav() {
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("cancelled.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 10_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 1000,
    };
    let recorder = Recorder::start(Box::new(source), cfg, None).await.unwrap();

    sink.push(loud_block(8000)).await.unwrap();
    recorder.cancel().await.unwrap();
    sink.close();

    assert!(!wav_path.exists(), "cancel should not write a wav file");
}

#[tokio::test]
async fn oneshot_mode_stops_on_silence() {
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("oneshot.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Oneshot,
        max_duration_ms: 30_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 500, // 0.5s silence to trigger
    };
    let recorder = Recorder::start(Box::new(source), cfg, None).await.unwrap();

    // 1s of loud audio, then 1s of silence
    sink.push(loud_block(16_000)).await.unwrap();
    sink.push(silent_block(16_000)).await.unwrap();

    // The recorder should auto-finalize. Wait for it.
    let finalize = recorder.wait_for_finalize(&wav_path).await.unwrap();
    sink.close();

    // Blocks are processed whole: the loud block (16_000) is appended, then the
    // entire silent block (16_000) is appended before the silence gate is
    // evaluated at the block boundary — where the trailing 8000-sample window is
    // all-zero and fires the auto-stop. So the recording is exactly the two
    // blocks: 32_000 samples = 2000 ms. Pinning the exact stop point catches a
    // regression that fires the gate a block too early (on the loud audio, ~1000
    // ms) or one that never fires (the test would hang, since the sink isn't
    // closed until after finalize — auto-stop firing at all is what lets
    // `wait_for_finalize` return).
    assert_eq!(
        finalize.duration_ms, 2000,
        "oneshot must auto-stop exactly at the silent-block boundary (2.0s)"
    );
    assert_eq!(
        finalize.samples_written, 32_000,
        "the loud + silent blocks (32_000 samples) must all be written"
    );
    assert!(wav_path.exists());
    let (samples, _) = wav::read_wav(&wav_path).unwrap();
    assert_eq!(samples.len(), 32_000, "WAV must carry all 32_000 samples");
    // The first half is the loud signal (peak well above the gate); the trailing
    // silence (last 8000 samples) is what tripped the auto-stop — both must be
    // present, proving the gate fired on the quiet tail, not on the loud audio.
    assert!(
        peak_of(&samples[..16_000]) > 10_000,
        "the loud lead-in must survive, got peak {}",
        peak_of(&samples[..16_000])
    );
    assert_eq!(
        peak_of(&samples[24_000..]),
        0,
        "the trailing 0.5s window that tripped the gate must be true silence"
    );
}

#[tokio::test]
async fn duration_mode_stops_after_n_seconds() {
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("dur.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Duration { secs: 1 },
        max_duration_ms: 30_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 5000,
    };
    let recorder = Recorder::start(Box::new(source), cfg, None).await.unwrap();

    // Feed plenty of loud samples; recorder should auto-stop at 1s.
    let pump = tokio::spawn({
        let sink = sink.clone();
        async move {
            for _ in 0..20 {
                if sink.push(loud_block(1600)).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    });

    let finalize = recorder.wait_for_finalize(&wav_path).await.unwrap();
    pump.abort();
    sink.close();

    assert!((finalize.duration_ms - 1000).abs() < 200);
}

#[tokio::test]
async fn max_duration_truncates_a_runaway_recording() {
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("max.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 500, // 0.5s cap
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 5000,
    };
    let recorder = Recorder::start(Box::new(source), cfg, None).await.unwrap();

    let pump = tokio::spawn({
        let sink = sink.clone();
        async move {
            for _ in 0..10 {
                if sink.push(loud_block(1600)).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    });

    let finalize = recorder.wait_for_finalize(&wav_path).await.unwrap();
    pump.abort();
    sink.close();

    assert!(finalize.duration_ms <= 600);
}

#[tokio::test]
async fn pause_discards_audio_until_resume() {
    // Verifies the core pause/resume contract: audio that arrives while paused
    // is drained from the source (so it can't back up) but not recorded, and
    // recording continues seamlessly into the same file after resume.
    //
    // Timeline: capture 0.5s, pause, push 1.0s (must be discarded), resume,
    // capture 0.5s. Final recording should be ~1.0s, never ~2.0s.
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("paused.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 30_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 10_000, // never auto-stop on silence during this test
    };
    let recorder = Recorder::start(Box::new(source), cfg, None).await.unwrap();

    // `settle` lets the recorder task drain whichever single channel is ready
    // before the next command/push, so the two channels never race within one
    // `select!` iteration. Matches the sleep-based sequencing used elsewhere in
    // this suite; tolerance below absorbs scheduling jitter.
    let settle = || tokio::time::sleep(Duration::from_millis(100));

    sink.push(loud_block(8000)).await.unwrap(); // 0.5s captured
    settle().await;

    recorder.pause().await.unwrap();
    settle().await;
    sink.push(loud_block(16_000)).await.unwrap(); // 1.0s — drained while paused, discarded
    settle().await;

    recorder.resume().await.unwrap();
    settle().await;
    sink.push(loud_block(8000)).await.unwrap(); // 0.5s captured
    settle().await;
    sink.close();

    let result = recorder.wait_for_finalize(&wav_path).await.unwrap();
    assert!(
        (result.duration_ms - 1000).abs() < 150,
        "paused audio should be discarded: expected ~1000ms, got {}ms",
        result.duration_ms
    );
    let (samples, _) = wav::read_wav(&wav_path).unwrap();
    assert!(
        samples.len() < 24_000,
        "captured {} samples — the 1.0s paused block was not discarded",
        samples.len()
    );
}

#[tokio::test]
async fn prepend_samples_lead_the_recording() {
    // Pre-roll contract: samples handed to `start_with_prepend` appear at the
    // very front of the finalized WAV, ahead of anything captured live.
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("preroll.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 30_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 10_000,
    };
    // 0.25s of pre-roll (4000 samples), distinct marker value.
    let preroll: Vec<i16> = vec![123; 4000];
    let recorder = Recorder::start_with_prepend(Box::new(source), cfg, None, preroll.clone())
        .await
        .unwrap();

    sink.push(loud_block(8000)).await.unwrap(); // 0.5s live
    sink.close();

    let result = recorder.wait_for_finalize(&wav_path).await.unwrap();
    // Duration should reflect pre-roll (0.25s) + live (0.5s) ≈ 0.75s.
    assert!(
        (result.duration_ms - 750).abs() < 100,
        "duration was {}ms (expected ~750)",
        result.duration_ms
    );

    let (samples, _) = wav::read_wav(&wav_path).unwrap();
    assert!(samples.len() >= 12_000 - 100);
    // The first 4000 samples must be the pre-roll marker, in order, ahead of
    // the captured audio.
    assert_eq!(&samples[..4000], &preroll[..]);
}

#[tokio::test]
async fn duration_counts_fresh_capture_excluding_preroll() {
    // A3: a `Duration { secs }` take must yield `secs` seconds of new audio with
    // the pre-roll added on top, not counted inside the requested duration. With
    // 0.5s of pre-roll and Duration { secs: 1 }, the result is ~1.5s: 1.0s of
    // fresh capture beyond the 0.5s lead-in.
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("dur_preroll.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Duration { secs: 1 },
        max_duration_ms: 30_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 5000,
    };
    // 0.5s of pre-roll (8000 samples at 16 kHz), distinct marker value.
    let preroll: Vec<i16> = vec![123; 8000];
    let recorder = Recorder::start_with_prepend(Box::new(source), cfg, None, preroll.clone())
        .await
        .unwrap();

    // Feed plenty of loud samples; the recorder must auto-stop after 1.0s of
    // fresh capture (not 0.5s, which is what the pre-roll would force if counted).
    let pump = tokio::spawn({
        let sink = sink.clone();
        async move {
            for _ in 0..30 {
                if sink.push(loud_block(1600)).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    });

    let finalize = recorder.wait_for_finalize(&wav_path).await.unwrap();
    pump.abort();
    sink.close();

    // Pre-roll (0.5s) + fresh (1.0s) ≈ 1.5s.
    assert!(
        (finalize.duration_ms - 1500).abs() < 200,
        "duration was {}ms (expected ~1500: 0.5s pre-roll + 1.0s fresh)",
        finalize.duration_ms
    );

    let (samples, _) = wav::read_wav(&wav_path).unwrap();
    // The fresh portion (everything after the pre-roll marker) must be ~1.0s.
    let fresh = samples.len() - preroll.len();
    assert!(
        (fresh as i64 - 16_000).abs() < 3_200,
        "fresh capture was {fresh} samples (expected ~16000 = 1.0s)"
    );
    // Pre-roll still leads the recording, unchanged.
    assert_eq!(&samples[..8000], &preroll[..]);
}

#[tokio::test]
async fn duration_with_no_preroll_unchanged() {
    // Back-compat: with no pre-roll, Duration { secs: 1 } is exactly 1s — the A3
    // accounting offset is zero and behaves the same as a plain start.
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("dur_noprl.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Duration { secs: 1 },
        max_duration_ms: 30_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 5000,
    };
    // Empty prepend == plain start; proves the no-pre-roll path is untouched.
    let recorder = Recorder::start_with_prepend(Box::new(source), cfg, None, Vec::new())
        .await
        .unwrap();

    let pump = tokio::spawn({
        let sink = sink.clone();
        async move {
            for _ in 0..20 {
                if sink.push(loud_block(1600)).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    });

    let finalize = recorder.wait_for_finalize(&wav_path).await.unwrap();
    pump.abort();
    sink.close();

    assert!(
        (finalize.duration_ms - 1000).abs() < 200,
        "no-preroll duration was {}ms (expected ~1000)",
        finalize.duration_ms
    );
}

#[tokio::test]
async fn max_duration_bounds_fresh_portion_not_preroll() {
    // A3: the max_duration_ms ceiling bounds the fresh portion; the prepended
    // pre-roll doesn't trip it early. With a 0.5s cap and 0.5s of pre-roll, the
    // result is ~1.0s (0.5s pre-roll + 0.5s fresh), never ~0.5s.
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("max_preroll.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 500, // 0.5s cap on fresh capture
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 5000,
    };
    // 0.5s of pre-roll (8000 samples).
    let preroll: Vec<i16> = vec![123; 8000];
    let recorder = Recorder::start_with_prepend(Box::new(source), cfg, None, preroll.clone())
        .await
        .unwrap();

    let pump = tokio::spawn({
        let sink = sink.clone();
        async move {
            for _ in 0..20 {
                if sink.push(loud_block(1600)).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    });

    let finalize = recorder.wait_for_finalize(&wav_path).await.unwrap();
    pump.abort();
    sink.close();

    // Pre-roll (0.5s) + capped fresh (0.5s) ≈ 1.0s, well above the 0.5s cap that
    // would result if pre-roll counted against it.
    assert!(
        (finalize.duration_ms - 1000).abs() < 200,
        "duration was {}ms (expected ~1000: 0.5s pre-roll + 0.5s capped fresh)",
        finalize.duration_ms
    );
    let (samples, _) = wav::read_wav(&wav_path).unwrap();
    let fresh = samples.len() - preroll.len();
    assert!(
        (fresh as i64 - 8_000).abs() < 2_000,
        "fresh portion was {fresh} samples (expected ~8000 = 0.5s cap)"
    );
}

#[tokio::test]
async fn max_duration_with_no_preroll_unchanged() {
    // Back-compat: with no pre-roll the cap behaves the same — a runaway Hold
    // recording is truncated at the 0.5s ceiling.
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("max_noprl.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 500,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 5000,
    };
    let recorder = Recorder::start_with_prepend(Box::new(source), cfg, None, Vec::new())
        .await
        .unwrap();

    let pump = tokio::spawn({
        let sink = sink.clone();
        async move {
            for _ in 0..10 {
                if sink.push(loud_block(1600)).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    });

    let finalize = recorder.wait_for_finalize(&wav_path).await.unwrap();
    pump.abort();
    sink.close();

    assert!(
        finalize.duration_ms <= 600,
        "no-preroll cap should hold at ~0.5s, got {}ms",
        finalize.duration_ms
    );
}

#[tokio::test]
async fn empty_prepend_matches_plain_start() {
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("empty_prepend.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 10_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 1000,
    };
    let recorder = Recorder::start_with_prepend(Box::new(source), cfg, None, Vec::new())
        .await
        .unwrap();
    sink.push(loud_block(8000)).await.unwrap();
    sink.close();
    let result = recorder.wait_for_finalize(&wav_path).await.unwrap();
    assert!((result.duration_ms - 500).abs() < 50);
    let (samples, _) = wav::read_wav(&wav_path).unwrap();
    assert!(samples.len() >= 8000 - 100);
}

#[tokio::test]
async fn snapshot_returns_captured_samples_without_stopping() {
    // Streaming-preview contract: `snapshot()` returns a clone of the audio
    // captured so far while leaving capture running, so a later snapshot sees
    // more, and the final WAV still contains everything.
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("snap.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 30_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 10_000, // never auto-stop during this test
    };
    let recorder = Recorder::start(Box::new(source), cfg, None).await.unwrap();

    let settle = || tokio::time::sleep(Duration::from_millis(100));

    sink.push(loud_block(8000)).await.unwrap(); // 0.5s
    settle().await;
    let snap1 = recorder.snapshot().await.unwrap();
    assert!(
        (snap1.len() as i64 - 8000).abs() < 200,
        "first snapshot had {} samples (expected ~8000)",
        snap1.len()
    );

    // Capture must continue after the snapshot — push more and snapshot again.
    sink.push(loud_block(8000)).await.unwrap(); // 1.0s total
    settle().await;
    let snap2 = recorder.snapshot().await.unwrap();
    assert!(
        snap2.len() > snap1.len(),
        "second snapshot ({}) should exceed first ({})",
        snap2.len(),
        snap1.len()
    );

    sink.close();
    let result = recorder.wait_for_finalize(&wav_path).await.unwrap();
    // The recording was never disturbed by the snapshots — full ~1.0s survives.
    assert!(
        (result.duration_ms - 1000).abs() < 100,
        "snapshots should not affect the recording: got {}ms",
        result.duration_ms
    );
}

#[tokio::test]
async fn snapshot_tail_bounds_clone_and_reports_total() {
    // The streaming preview relies on `snapshot_tail` to clone only the last
    // `max_tail` samples (constant per-tick cost) while still learning the full
    // captured length so it can throttle on newly-accumulated audio.
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("tail.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 30_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 10_000, // never auto-stop during this test
    };
    let recorder = Recorder::start(Box::new(source), cfg, None).await.unwrap();
    let settle = || tokio::time::sleep(Duration::from_millis(100));

    // Capture ~24000 samples (1.5s) total.
    sink.push(loud_block(24_000)).await.unwrap();
    settle().await;

    // Ask for only the last 8000 samples.
    let (total, tail) = recorder.snapshot_tail(8_000).await.unwrap();
    assert!(
        (total as i64 - 24_000).abs() < 400,
        "total should reflect the full ~24000 captured, got {total}"
    );
    assert_eq!(
        tail.len(),
        8_000,
        "tail must be bounded to max_tail when the buffer is larger"
    );

    // max_tail = 0 means "the whole buffer".
    let (total_all, all) = recorder.snapshot_tail(0).await.unwrap();
    assert_eq!(all.len(), total_all, "max_tail=0 returns the full buffer");
    assert!(all.len() >= tail.len());
    // The full snapshot must carry the *actual* captured loud audio, not a
    // correctly-sized but zeroed/wrong buffer. `loud_block` peaks near 20_000, so
    // a faithful clone has a large peak; its trailing 8000 samples must equal the
    // bounded tail (same window over the same buffer).
    assert!(
        peak_of(&all) > 10_000,
        "the full snapshot must contain the captured loud signal, got peak {}",
        peak_of(&all)
    );
    assert_eq!(
        &all[all.len() - tail.len()..],
        &tail[..],
        "the tail snapshot must equal the last max_tail samples of the full buffer"
    );

    // A max_tail larger than the buffer returns everything (never panics).
    let (_t, big) = recorder.snapshot_tail(10_000_000).await.unwrap();
    assert_eq!(big.len(), total_all);

    sink.close();
    let _ = recorder.wait_for_finalize(&wav_path).await.unwrap();
}

#[tokio::test]
async fn config_forwards_full_source_format() {
    // `audio_config()` must faithfully forward the *source's* format — both rate
    // AND channels — not hardcode the 16 kHz mono default. Build the source from
    // a deliberately non-canonical config (44.1 kHz stereo) so a recorder that
    // mangled either field, or echoed a hardcoded default, is caught.
    let non_default = AudioConfig {
        sample_rate: SampleRate::HZ_44_1K,
        channels: Channels::STEREO,
    };
    let (source, _sink) = SyntheticSource::new(non_default);
    let recorder = Recorder::start(Box::new(source), RecorderConfig::default(), None)
        .await
        .unwrap();
    let reported = recorder.audio_config();
    assert_eq!(reported.sample_rate, SampleRate::HZ_44_1K);
    assert_eq!(reported.channels, Channels::STEREO);
    assert_eq!(reported, non_default);
    let _ = recorder.cancel().await;
}

#[tokio::test]
async fn config_reports_canonical_default_format() {
    // The default synthetic source carries Phoneme's canonical capture format —
    // 16 kHz mono — and the recorder must report exactly that.
    let (source, _sink) = make_synthetic();
    let recorder = Recorder::start(Box::new(source), RecorderConfig::default(), None)
        .await
        .unwrap();
    assert_eq!(recorder.audio_config().sample_rate, SampleRate::HZ_16K);
    assert_eq!(recorder.audio_config().channels, Channels::MONO);
    let _ = recorder.cancel().await;
}

#[tokio::test]
async fn normalize_boosts_quiet_recording_on_finalize() {
    // With normalization enabled, a quiet capture (peak ~1000) is lifted toward
    // the -1 dBFS ceiling (~29204) before the WAV is written.
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("quiet.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 10_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 1000,
    };
    let recorder = Recorder::start(Box::new(source), cfg, None)
        .await
        .unwrap()
        .with_normalize(-1.0);

    sink.push(quiet_block(16_000)).await.unwrap();
    sink.close();

    recorder.wait_for_finalize(&wav_path).await.unwrap();
    let (samples, _) = wav::read_wav(&wav_path).unwrap();
    // The -1 dBFS ceiling is ~29204; the boosted peak should sit just under it,
    // proving the audio was lifted toward — but never past — full scale.
    let peak = peak_of(&samples);
    assert!(
        (25_000..=29_205).contains(&peak),
        "quiet recording should be normalized up to just under -1 dBFS, got peak {peak}"
    );
}

#[tokio::test]
async fn device_loss_finalizes_partial_and_flags_device_lost() {
    // A1: when the source fails mid-capture (mic unplugged), the recorder still
    // writes the audio captured up to the drop, and the result is flagged
    // `device_lost` so the daemon can tell the user why capture ended.
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("dropped.wav");
    // 1.0s of buffered audio, then the source errors.
    let source = FailingSource::new(vec![loud_block(8000), loud_block(8000)]);
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 30_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 10_000,
    };
    let recorder = Recorder::start(Box::new(source), cfg, None).await.unwrap();

    // The loop breaks on the Err on its own; `wait_for_finalize` awaits that
    // natural completion (no Stop command involved).
    let result = recorder.wait_for_finalize(&wav_path).await.unwrap();
    assert!(
        result.device_lost,
        "a mid-capture device failure must set device_lost"
    );
    // The partial take is still saved exactly as a normal recording.
    assert!(
        wav_path.exists(),
        "the partial recording must still be written"
    );
    let (samples, _) = wav::read_wav(&wav_path).unwrap();
    assert!(
        samples.len() >= 16_000 - 200,
        "the ~1s captured before the drop must survive, got {}",
        samples.len()
    );
}

#[tokio::test]
async fn normal_stop_does_not_flag_device_lost() {
    // The inverse contract: a clean user stop must never look like a disconnect,
    // or the UI would toast "microphone disconnected" on every ordinary stop.
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("clean.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 30_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 10_000,
    };
    let recorder = Recorder::start(Box::new(source), cfg, None).await.unwrap();

    sink.push(loud_block(8000)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    // An explicit stop — the user-initiated path.
    let result = recorder.stop_and_finalize(&wav_path).await.unwrap();
    sink.close();
    assert!(
        !result.device_lost,
        "a normal user stop must not be reported as a device loss"
    );
    assert!(wav_path.exists());
}

#[tokio::test]
async fn clean_end_of_stream_does_not_flag_device_lost() {
    // The source closing cleanly (Ok(None)) — e.g. an auto-stop or the synthetic
    // sink being dropped — is also not a device loss.
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("eos.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 30_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 10_000,
    };
    let recorder = Recorder::start(Box::new(source), cfg, None).await.unwrap();
    sink.push(loud_block(8000)).await.unwrap();
    sink.close(); // clean end-of-stream → Ok(None)
    let result = recorder.wait_for_finalize(&wav_path).await.unwrap();
    assert!(
        !result.device_lost,
        "a clean end-of-stream must not be reported as a device loss"
    );
}

#[tokio::test]
async fn normalize_off_by_default_leaves_quiet_recording_quiet() {
    // Without `with_normalize`, the captured level is preserved verbatim.
    let dir = TempDir::new().unwrap();
    let wav_path = dir.path().join("quiet-asis.wav");
    let (source, sink) = make_synthetic();
    let cfg = RecorderConfig {
        mode: RecordingMode::Hold,
        max_duration_ms: 10_000,
        silence_threshold_dbfs: -45.0,
        silence_window_ms: 1000,
    };
    let recorder = Recorder::start(Box::new(source), cfg, None).await.unwrap();

    sink.push(quiet_block(16_000)).await.unwrap();
    sink.close();

    recorder.wait_for_finalize(&wav_path).await.unwrap();
    let (samples, _) = wav::read_wav(&wav_path).unwrap();
    assert!(
        peak_of(&samples) < 1_200,
        "default-off must leave the quiet recording as captured, got peak {}",
        peak_of(&samples)
    );
}
