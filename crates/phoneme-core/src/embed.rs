use crate::error::{Error, Result};
use ort::{
    inputs,
    session::{builder::GraphOptimizationLevel, Session},
    value::Tensor,
};
use std::path::Path;
use std::sync::Mutex;
use tokenizers::{Tokenizer, TruncationParams};

/// Maximum input length (in tokens) fed to the embedding model.
///
/// `all-MiniLM-L6-v2` is a BERT model with 512 learned position embeddings and
/// was fine-tuned at a 256-token sequence length. The tokenizer.json shipped
/// with the model has truncation **disabled**, so without configuring it here a
/// long transcript produces a sequence in the thousands of tokens — which makes
/// the ONNX session either error (the embedding is then silently skipped,
/// leaving the recording unsearchable) or return a meaningless vector. We
/// truncate to the model's training length so every transcript embeds cleanly.
pub(crate) const EMBED_MAX_TOKENS: usize = 256;

/// Truncation policy applied to the tokenizer at load. Pulled out as a function
/// so it can be unit-tested without the (unbundled) ONNX model + tokenizer.
pub(crate) fn embedding_truncation() -> TruncationParams {
    TruncationParams {
        max_length: EMBED_MAX_TOKENS,
        ..Default::default()
    }
}

/// Wrapper around the ONNX model and tokenizer for generating embeddings.
///
/// Phoneme uses `all-MiniLM-L6-v2` by default (384-dimensional output).
pub struct Embedder {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
}

impl Embedder {
    /// Loads the tokenizer and ONNX session from the given directory.
    /// Expects `model.onnx` and `tokenizer.json` to be present.
    pub fn new(model_dir: &Path) -> Result<Self> {
        let tokenizer_path = model_dir.join("tokenizer.json");
        let model_path = model_dir.join("model.onnx");

        if !tokenizer_path.exists() {
            return Err(Error::Internal(format!(
                "Tokenizer not found at {}",
                tokenizer_path.display()
            )));
        }
        if !model_path.exists() {
            return Err(Error::Internal(format!(
                "ONNX model not found at {}",
                model_path.display()
            )));
        }

        let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| Error::Internal(format!("Failed to load tokenizer: {}", e)))?;
        // Cap sequence length to the model's limit — see EMBED_MAX_TOKENS. Without
        // this, long transcripts overflow BERT's 512 positions and fail to embed.
        tokenizer
            .with_truncation(Some(embedding_truncation()))
            .map_err(|e| {
                Error::Internal(format!("Failed to configure tokenizer truncation: {}", e))
            })?;

        let session = Session::builder()
            .map_err(|e| Error::Internal(format!("Failed to build ORT session: {}", e)))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| Error::Internal(format!("Failed to set optimization: {}", e)))?
            .with_intra_threads(1)
            .map_err(|e| Error::Internal(format!("Failed to set threads: {}", e)))?
            .commit_from_file(&model_path)
            .map_err(|e| Error::Internal(format!("Failed to load ONNX model: {}", e)))?;

        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
        })
    }

    /// Generates a semantic embedding vector for the given text.
    ///
    /// The process:
    /// 1. Tokenize the input string.
    /// 2. Feed `input_ids`, `attention_mask`, and `token_type_ids` to the model.
    /// 3. Mean-pool the token embeddings (ignoring padding) to get a single vector.
    /// 4. L2-normalize the vector.
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| Error::Internal(format!("Failed to tokenize: {}", e)))?;

        let seq_len = encoding.get_ids().len();

        let input_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let attention_mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&m| m as i64)
            .collect();
        let token_type_ids: Vec<i64> = encoding.get_type_ids().iter().map(|&t| t as i64).collect();

        // Convert to ndarray with shape (batch_size=1, seq_len)
        let input_ids_array = ndarray::Array2::from_shape_vec((1, seq_len), input_ids)
            .map_err(|e| Error::Internal(format!("Array error: {}", e)))?;
        let attention_mask_array =
            ndarray::Array2::from_shape_vec((1, seq_len), attention_mask.clone())
                .map_err(|e| Error::Internal(format!("Array error: {}", e)))?;
        let token_type_ids_array = ndarray::Array2::from_shape_vec((1, seq_len), token_type_ids)
            .map_err(|e| Error::Internal(format!("Array error: {}", e)))?;

        let input_ids_tensor = Tensor::from_array(input_ids_array)
            .map_err(|e| Error::Internal(format!("Tensor error: {}", e)))?;
        let attention_mask_tensor = Tensor::from_array(attention_mask_array)
            .map_err(|e| Error::Internal(format!("Tensor error: {}", e)))?;
        let token_type_ids_tensor = Tensor::from_array(token_type_ids_array)
            .map_err(|e| Error::Internal(format!("Tensor error: {}", e)))?;

        let mut session = self
            .session
            .lock()
            .map_err(|e| Error::Internal(format!("embedder session mutex poisoned: {e}")))?;
        let outputs = session
            .run(inputs![
                "input_ids" => input_ids_tensor,
                "attention_mask" => attention_mask_tensor,
                "token_type_ids" => token_type_ids_tensor,
            ])
            .map_err(|e| Error::Internal(format!("Inference failed: {}", e)))?;

        // Extract last_hidden_state. Usually it's the first output or named "last_hidden_state"
        let (shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| Error::Internal(format!("Extract tensor error: {}", e)))?;

        // shape is usually [1, seq_len, hidden_size]
        // data is &[f32]
        let hidden_size = shape[2] as usize;

        // Perform mean pooling using attention mask
        let mut pooled = vec![0.0f32; hidden_size];
        let mut mask_sum = 0.0f32;

        for i in 0..seq_len {
            let mask = attention_mask[i] as f32;
            if mask > 0.0 {
                mask_sum += mask;
                for j in 0..hidden_size {
                    // data layout is [batch, seq, hidden].
                    // index = i * hidden_size + j
                    pooled[j] += data[i * hidden_size + j] * mask;
                }
            }
        }

        // Divide by sum of mask
        for val in &mut pooled {
            *val /= mask_sum.max(1e-9);
        }

        // L2 Normalize
        let norm: f32 = pooled.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for val in &mut pooled {
                *val /= norm;
            }
        }

        Ok(pooled)
    }

    /// Embed a transcript as a set of sentence-aware, overlapping chunks.
    ///
    /// Returns one L2-normalized vector per chunk produced by
    /// [`crate::chunk::chunk_transcript`]. This is the ingest path for semantic
    /// search: storing per-chunk vectors (instead of one mean-pooled vector for
    /// the whole transcript) is what lets a query paraphrasing a single spoken
    /// idea match on that idea's *own* vector rather than an averaged-out blur of
    /// the entire note — the central paraphrase-recall fix.
    ///
    /// Each chunk is embedded with the same model + normalization as a query, so
    /// query and document vectors are directly comparable by cosine. An empty or
    /// whitespace-only transcript yields no chunks (and so no embeddings).
    pub fn embed_chunks(&self, text: &str) -> Result<Vec<Vec<f32>>> {
        let chunks = crate::chunk::chunk_transcript(text);
        let mut out = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            out.push(self.embed(&chunk)?);
        }
        Ok(out)
    }

    /// Computes cosine similarity between two L2-normalized vectors.
    pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenizers::TruncationStrategy;

    #[test]
    fn truncation_caps_at_model_limit() {
        // The embedder must truncate long inputs to the model's trained length,
        // otherwise long transcripts overflow BERT's position embeddings and
        // fail to embed (the recording then becomes unsearchable).
        let t = embedding_truncation();
        assert_eq!(t.max_length, EMBED_MAX_TOKENS);
        assert_eq!(EMBED_MAX_TOKENS, 256, "all-MiniLM-L6-v2 was trained at 256");
        assert!(
            matches!(t.strategy, TruncationStrategy::LongestFirst),
            "longest-first truncation keeps the start of the transcript"
        );
    }
}
