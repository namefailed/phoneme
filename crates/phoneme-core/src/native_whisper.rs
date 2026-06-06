use anyhow::{Context, Result};
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct NativeWhisperProvider {
    context: WhisperContext,
}

impl NativeWhisperProvider {
    pub fn new(model_path: &Path) -> Result<Self> {
        let params = WhisperContextParameters::default();
        let context =
            WhisperContext::new_with_params(model_path.to_string_lossy().as_ref(), params)
                .context("failed to load native whisper model")?;
        Ok(Self { context })
    }

    fn read_wav_f32(path: &Path) -> Result<Vec<f32>> {
        let mut reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int => {
                let max_val = 1_f32 / (1 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .into_samples::<i32>()
                    .filter_map(Result::ok)
                    .map(|s| s as f32 * max_val)
                    .collect()
            }
            hound::SampleFormat::Float => reader
                .into_samples::<f32>()
                .filter_map(Result::ok)
                .collect(),
        };
        Ok(samples)
    }
}

#[async_trait::async_trait]
impl crate::transcription::TranscriptionProvider for NativeWhisperProvider {
    fn is_native(&self) -> bool {
        true
    }

    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String> {
        let samples = Self::read_wav_f32(audio_path)?;

        let mut state = self
            .context
            .create_state()
            .context("failed to create whisper state")?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        if let Some(lang) = language {
            params.set_language(Some(lang));
        } else {
            params.set_language(Some("en"));
        }

        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state
            .full(params, &samples)
            .context("native whisper transcription failed")?;

        let num_segments = state.full_n_segments().context("failed to get segments")?;
        let mut result = String::new();
        for i in 0..num_segments {
            if let Ok(text) = state.full_get_segment_text(i) {
                result.push_str(&text);
                result.push(' ');
            }
        }

        Ok(result.trim().to_string())
    }
}
