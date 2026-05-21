use phoneme_audio::format::{AudioConfig, Channels, SampleRate};
use phoneme_audio::wav;
use tempfile::TempDir;

fn synth_sine(seconds: f32, freq: f32, sample_rate: u32) -> Vec<i16> {
    let n = (seconds * sample_rate as f32) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            (32000.0 * (2.0 * std::f32::consts::PI * freq * t).sin()) as i16
        })
        .collect()
}

#[test]
fn write_then_read_round_trips_samples() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sine.wav");
    let samples = synth_sine(1.0, 440.0, 16_000);
    wav::write_wav(&path, &samples, AudioConfig::phoneme_default()).unwrap();

    let (read_samples, cfg) = wav::read_wav(&path).unwrap();
    assert_eq!(cfg.sample_rate, SampleRate::HZ_16K);
    assert_eq!(cfg.channels, Channels::MONO);
    assert_eq!(read_samples.len(), samples.len());
    // Allow small rounding drift; should be exact for i16.
    assert_eq!(read_samples, samples);
}

#[test]
fn write_creates_parent_directories() {
    let dir = TempDir::new().unwrap();
    let nested = dir.path().join("2026-05-19").join("143500823.wav");
    let samples = synth_sine(0.1, 440.0, 16_000);
    wav::write_wav(&nested, &samples, AudioConfig::phoneme_default()).unwrap();
    assert!(nested.exists());
}

#[test]
fn write_empty_buffer_is_ok() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("empty.wav");
    let samples: Vec<i16> = vec![];
    wav::write_wav(&path, &samples, AudioConfig::phoneme_default()).unwrap();
    let (read_samples, _) = wav::read_wav(&path).unwrap();
    assert!(read_samples.is_empty());
}

#[test]
fn duration_matches_sample_count() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("d.wav");
    let samples = synth_sine(2.5, 440.0, 16_000);
    wav::write_wav(&path, &samples, AudioConfig::phoneme_default()).unwrap();
    let ms = wav::duration_ms(&path).unwrap();
    // 2.5s @ 16kHz = 40000 samples / 16000 = 2.5s = 2500ms (±1ms tolerance)
    assert!((2490..=2510).contains(&ms), "got {ms}ms");
}

#[test]
fn read_nonexistent_file_is_error() {
    let err = wav::read_wav(std::path::Path::new("/no/such/file.wav")).unwrap_err();
    assert!(
        format!("{err}").to_lowercase().contains("no such")
            || format!("{err}").to_lowercase().contains("not found")
            || format!("{err}").to_lowercase().contains("cannot")
            || format!("{err}")
                .to_lowercase()
                .contains("system cannot find")
    );
}
