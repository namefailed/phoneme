use phoneme_audio::format::{AudioConfig, SampleRate};
use phoneme_audio::recorder::{RecorderConfig, RecordingMode};
use phoneme_audio::source::{SyntheticSink, SyntheticSource};
use phoneme_audio::wav;
use phoneme_audio::Recorder;
use std::time::Duration;
use tempfile::TempDir;

fn loud_block(samples: usize) -> Vec<i16> {
    (0..samples)
        .map(|i| ((i as f32 * 0.05).sin() * 20_000.0) as i16)
        .collect()
}

fn silent_block(samples: usize) -> Vec<i16> {
    vec![0; samples]
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
    let recorder = Recorder::start(Box::new(source), cfg).await.unwrap();

    sink.push(loud_block(8000)).await.unwrap(); // 0.5s
    sink.push(loud_block(8000)).await.unwrap(); // 1.0s total
    sink.close(); // signal end-of-stream — recorder will drain & exit naturally

    // Use `wait_for_finalize` (which awaits natural task completion via
    // source-exhaustion) rather than `stop_and_finalize`. The latter races
    // the cmd-channel Stop against the source channel inside the recorder's
    // unbiased `tokio::select!` — in production that race is acceptable
    // (CPAL never closes, so Stop is the only exit), but in this synthetic
    // test the race causes a flaky duration assertion.
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
    let recorder = Recorder::start(Box::new(source), cfg).await.unwrap();

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
    let recorder = Recorder::start(Box::new(source), cfg).await.unwrap();

    // 1s of loud audio, then 1s of silence
    sink.push(loud_block(16_000)).await.unwrap();
    sink.push(silent_block(16_000)).await.unwrap();

    // The recorder should auto-finalize. Wait for it.
    let finalize = recorder.wait_for_finalize(&wav_path).await.unwrap();
    sink.close();

    assert!(finalize.duration_ms < 30_000);
    assert!(wav_path.exists());
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
    let recorder = Recorder::start(Box::new(source), cfg).await.unwrap();

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
    let recorder = Recorder::start(Box::new(source), cfg).await.unwrap();

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
async fn config_is_canonical_format() {
    let (source, _sink) = make_synthetic();
    let cfg = RecorderConfig::default();
    let recorder = Recorder::start(Box::new(source), cfg).await.unwrap();
    assert_eq!(recorder.audio_config().sample_rate, SampleRate::HZ_16K);
    let _ = recorder.cancel().await;
}
