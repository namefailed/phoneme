use phoneme_audio::silence::SilenceDetector;

fn synth_silence(samples: usize) -> Vec<i16> {
    vec![0; samples]
}

fn synth_loud(samples: usize) -> Vec<i16> {
    (0..samples)
        .map(|i| ((i as f32 * 0.05).sin() * 20_000.0) as i16)
        .collect()
}

#[test]
fn empty_input_does_not_trigger() {
    let mut det = SilenceDetector::new(-45.0, 1000, 16_000);
    assert!(!det.is_silent());
    det.push(&[]);
    assert!(!det.is_silent());
}

#[test]
fn loud_input_does_not_trigger() {
    // 2 seconds of loud signal at 16kHz; window is 1 second; should never trigger.
    let mut det = SilenceDetector::new(-45.0, 1000, 16_000);
    det.push(&synth_loud(32_000));
    assert!(!det.is_silent());
}

#[test]
fn silent_input_for_full_window_triggers() {
    // 1.5 seconds of silence; window is 1s → triggers.
    let mut det = SilenceDetector::new(-45.0, 1000, 16_000);
    det.push(&synth_silence(24_000));
    assert!(det.is_silent());
}

#[test]
fn silent_input_below_window_does_not_trigger() {
    // 500ms of silence; window is 1s → does NOT trigger yet.
    let mut det = SilenceDetector::new(-45.0, 1000, 16_000);
    det.push(&synth_silence(8_000));
    assert!(!det.is_silent());
}

#[test]
fn loud_then_silent_takes_full_window_to_trigger() {
    let mut det = SilenceDetector::new(-45.0, 1000, 16_000);
    det.push(&synth_loud(8_000)); // 0.5s loud
    assert!(!det.is_silent());
    det.push(&synth_silence(8_000)); // 0.5s silent
    assert!(!det.is_silent()); // half window of silence — still loud overall
    det.push(&synth_silence(8_000)); // another 0.5s silent
    assert!(det.is_silent()); // now full window is silent
}

#[test]
fn reset_clears_history() {
    let mut det = SilenceDetector::new(-45.0, 1000, 16_000);
    det.push(&synth_silence(24_000));
    assert!(det.is_silent());
    det.reset();
    assert!(!det.is_silent());
}
