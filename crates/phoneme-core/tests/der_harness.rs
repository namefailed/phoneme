//! Diarization Error Rate (DER) harness (#10) — runs the local diarizer on an
//! audio fixture and scores its labels against a hand-checked RTTM reference,
//! using the pure metric in `phoneme_core::der`.
//!
//! Ignored by default: it needs the ~500 MB speakrs models and a fixture set, so
//! it is a manual / nightly tool, not a unit test. Run it like:
//!
//! ```text
//! PHONEME_DER_AUDIO=fixtures/meeting.wav \
//! PHONEME_DER_RTTM=fixtures/meeting.rttm \
//! PHONEME_DER_MAX=0.4 \
//!   cargo test -p phoneme-core --test der_harness -- --ignored --nocapture
//! ```
//!
//! It prints the full breakdown (missed / false-alarm / confusion) and fails when
//! the DER exceeds `PHONEME_DER_MAX` (default 0.5) — so a nightly job can gate on
//! "the local diarizer hasn't regressed past this budget on the fixture".

use phoneme_core::config::{DiarizationBackend, DiarizationConfig};
use phoneme_core::der::{compute_der, parse_rttm, DerSegment};
use phoneme_core::diarization::{run_local_diarization, LocalDiarizerCache};
use std::path::Path;

#[test]
#[ignore = "needs PHONEME_DER_AUDIO + PHONEME_DER_RTTM fixtures and the speakrs models"]
fn der_against_reference_rttm() {
    let audio = std::env::var("PHONEME_DER_AUDIO").expect("set PHONEME_DER_AUDIO to a .wav file");
    let rttm_path =
        std::env::var("PHONEME_DER_RTTM").expect("set PHONEME_DER_RTTM to a reference .rttm file");
    let max_der: f64 = std::env::var("PHONEME_DER_MAX")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.5);

    let reference = parse_rttm(&std::fs::read_to_string(&rttm_path).expect("read reference RTTM"));
    assert!(
        !reference.is_empty(),
        "reference RTTM {rttm_path} had no SPEAKER lines"
    );

    let cache = LocalDiarizerCache::new();
    let cfg = DiarizationConfig {
        provider: DiarizationBackend::Local,
        ..DiarizationConfig::default()
    };
    let diarized = run_local_diarization(Path::new(&audio), &cache, &cfg)
        .expect("local diarization should run on the fixture");
    let hypothesis = DerSegment::from_spans(&diarized.spans);

    let report = compute_der(&reference, &hypothesis);
    println!(
        "DER {:.3}  |  missed {:.2}s · false-alarm {:.2}s · confusion {:.2}s · reference {:.2}s",
        report.der, report.missed, report.false_alarm, report.confusion, report.total_reference,
    );
    assert!(
        report.der <= max_der,
        "DER {:.3} exceeded the {max_der:.3} budget",
        report.der,
    );
}
