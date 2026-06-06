use anyhow::Result;
use hound;
use speakrs::{ExecutionMode, OwnedDiarizationPipeline};
use std::path::Path;

pub fn load_audio_mono_16khz(path: &Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();

    // Simplistic float conversion for 16kHz mono.
    // In a real app we might need to resample/mixdown using a crate like `rubato`
    // but for whisper/phoneme we usually enforce 16kHz mono at the recorder level.
    let samples: Result<Vec<f32>, _> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect(),
        hound::SampleFormat::Float => reader.samples::<f32>().collect(),
    };

    Ok(samples?)
}

pub fn run_local_diarization(audio_path: &Path) -> Result<Vec<speakrs::segment::Segment>> {
    // We default to CPU for maximum compatibility on Windows without CUDA setup
    let mut pipeline = OwnedDiarizationPipeline::from_pretrained(ExecutionMode::Cpu)?;

    let audio = load_audio_mono_16khz(audio_path)?;
    let result = pipeline.run(&audio)?;

    // Return segments using a default step of 1s and duration of 1s for accurate boundaries
    let segments = result.discrete_diarization.to_segments(1.0, 1.0);
    Ok(segments)
}
