use crate::config::{EmbeddingPooling, SemanticSearchConfig};
use crate::error::{Error, Result};
use ort::{
    inputs,
    session::{builder::GraphOptimizationLevel, Session},
    value::Tensor,
};
use std::sync::Mutex;
use tokenizers::{Tokenizer, TruncationParams};

/// Truncation policy applied to the tokenizer at load. The tokenizer.json shipped
/// with most sentence-transformers has truncation **disabled**, so without this a
/// long transcript produces a multi-thousand-token sequence that overflows the
/// model's position embeddings and fails to embed (leaving the recording
/// unsearchable). Pulled out as a function so it can be unit-tested without the
/// (unbundled) ONNX model + tokenizer.
pub(crate) fn embedding_truncation(max_tokens: usize) -> TruncationParams {
    TruncationParams {
        max_length: max_tokens.max(1),
        ..Default::default()
    }
}

/// Reduce a model's per-token hidden states `data` (layout `[seq, hidden]`) to a
/// single L2-normalized sentence vector, per the chosen [`EmbeddingPooling`].
/// Pure (no ONNX/tokenizer) so the pooling math is unit-testable.
pub(crate) fn pool(
    data: &[f32],
    hidden_size: usize,
    seq_len: usize,
    attention_mask: &[i64],
    pooling: EmbeddingPooling,
) -> Vec<f32> {
    let mut pooled = match pooling {
        // `[CLS]` is token 0; take its hidden vector directly. Used by models
        // trained for CLS pooling rather than mean pooling.
        EmbeddingPooling::Cls => data
            .get(..hidden_size)
            .map(<[f32]>::to_vec)
            .unwrap_or_else(|| vec![0.0; hidden_size]),
        // Attention-mask-weighted average over real (non-pad) tokens — the
        // standard for MiniLM/MPNet/E5/BGE.
        EmbeddingPooling::Mean => {
            let mut p = vec![0.0f32; hidden_size];
            let mut mask_sum = 0.0f32;
            for i in 0..seq_len {
                let m = *attention_mask.get(i).unwrap_or(&0) as f32;
                if m > 0.0 {
                    mask_sum += m;
                    let base = i * hidden_size;
                    for j in 0..hidden_size {
                        p[j] += data[base + j] * m;
                    }
                }
            }
            for v in &mut p {
                *v /= mask_sum.max(1e-9);
            }
            p
        }
    };
    l2_normalize(&mut pooled);
    pooled
}

/// L2-normalize in place (a zero vector is left unchanged).
pub(crate) fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Wrapper around an ONNX sentence-transformer + tokenizer for generating
/// embeddings. The bundled default is `all-MiniLM-L6-v2` (384-dim, mean-pooled,
/// no prefixes), but every knob — max length, pooling, whether the model takes
/// `token_type_ids`, and the query/passage prefixes — is driven by
/// [`SemanticSearchConfig`], so a user can point `model_dir` at a different model
/// (E5, BGE, GTE, MPNet…) and have it work.
pub struct Embedder {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    pooling: EmbeddingPooling,
    /// Whether to feed a `token_type_ids` input (BERT-family yes; some E5 exports no).
    token_type_ids: bool,
    query_prefix: String,
    passage_prefix: String,
}

impl Embedder {
    /// Load the tokenizer + ONNX session from `cfg.model_dir` (expects
    /// `model.onnx` and `tokenizer.json`) and capture the per-model embedding
    /// knobs from `cfg`.
    pub fn new(cfg: &SemanticSearchConfig) -> Result<Self> {
        let model_dir = &cfg.model_dir;
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
        // Cap sequence length to the model's trained limit (see embedding_truncation).
        tokenizer
            .with_truncation(Some(embedding_truncation(cfg.max_tokens)))
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
            pooling: cfg.pooling,
            token_type_ids: cfg.token_type_ids,
            query_prefix: cfg.query_prefix.clone(),
            passage_prefix: cfg.passage_prefix.clone(),
        })
    }

    /// Embed a stored passage / transcript (applies the configured passage prefix).
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_with_prefix(text, &self.passage_prefix)
    }

    /// Embed a search query (applies the configured query prefix). For symmetric
    /// models (all-MiniLM) both prefixes are empty and this matches [`embed`].
    pub fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_with_prefix(text, &self.query_prefix)
    }

    /// Generate an L2-normalized embedding for `prefix + text`:
    /// tokenize → feed `input_ids`/`attention_mask` (+ `token_type_ids` when the
    /// model uses them) → pool the token hidden states per the configured strategy.
    fn embed_with_prefix(&self, text: &str, prefix: &str) -> Result<Vec<f32>> {
        // Instruction-tuned models (E5/BGE) expect a role prefix; for symmetric
        // models the prefix is empty, so this allocation is skipped.
        let prefixed;
        let input: &str = if prefix.is_empty() {
            text
        } else {
            prefixed = format!("{prefix}{text}");
            &prefixed
        };

        let encoding = self
            .tokenizer
            .encode(input, true)
            .map_err(|e| Error::Internal(format!("Failed to tokenize: {}", e)))?;
        let seq_len = encoding.get_ids().len();

        let input_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let attention_mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&m| m as i64)
            .collect();

        let input_ids_array = ndarray::Array2::from_shape_vec((1, seq_len), input_ids)
            .map_err(|e| Error::Internal(format!("Array error: {}", e)))?;
        let attention_mask_array =
            ndarray::Array2::from_shape_vec((1, seq_len), attention_mask.clone())
                .map_err(|e| Error::Internal(format!("Array error: {}", e)))?;
        let input_ids_tensor = Tensor::from_array(input_ids_array)
            .map_err(|e| Error::Internal(format!("Tensor error: {}", e)))?;
        let attention_mask_tensor = Tensor::from_array(attention_mask_array)
            .map_err(|e| Error::Internal(format!("Tensor error: {}", e)))?;

        let mut session = self
            .session
            .lock()
            .map_err(|e| Error::Internal(format!("embedder session mutex poisoned: {e}")))?;
        let outputs = if self.token_type_ids {
            let token_type_ids: Vec<i64> =
                encoding.get_type_ids().iter().map(|&t| t as i64).collect();
            let tt_array = ndarray::Array2::from_shape_vec((1, seq_len), token_type_ids)
                .map_err(|e| Error::Internal(format!("Array error: {}", e)))?;
            let tt_tensor = Tensor::from_array(tt_array)
                .map_err(|e| Error::Internal(format!("Tensor error: {}", e)))?;
            session
                .run(inputs![
                    "input_ids" => input_ids_tensor,
                    "attention_mask" => attention_mask_tensor,
                    "token_type_ids" => tt_tensor,
                ])
                .map_err(|e| Error::Internal(format!("Inference failed: {}", e)))?
        } else {
            session
                .run(inputs![
                    "input_ids" => input_ids_tensor,
                    "attention_mask" => attention_mask_tensor,
                ])
                .map_err(|e| Error::Internal(format!("Inference failed: {}", e)))?
        };

        let (shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| Error::Internal(format!("Extract tensor error: {}", e)))?;

        // Most exports emit `last_hidden_state` [1, seq, hidden] and expect us to
        // pool; some emit an already-pooled sentence vector [1, hidden] — handle
        // both so swapping models "just works".
        let pooled = if shape.len() == 2 {
            let hidden = shape[1] as usize;
            let mut v = data
                .get(..hidden)
                .map(<[f32]>::to_vec)
                .unwrap_or_else(|| vec![0.0; hidden]);
            l2_normalize(&mut v);
            v
        } else {
            let hidden_size = shape[2] as usize;
            pool(data, hidden_size, seq_len, &attention_mask, self.pooling)
        };

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
    fn truncation_caps_at_configured_limit() {
        // The embedder must truncate long inputs to the model's trained length,
        // otherwise long transcripts overflow the position embeddings and fail to
        // embed (the recording then becomes unsearchable). The cap is now
        // per-model configurable; a value of 0 is clamped to 1.
        let t = embedding_truncation(256);
        assert_eq!(t.max_length, 256);
        assert!(matches!(t.strategy, TruncationStrategy::LongestFirst));
        assert_eq!(embedding_truncation(128).max_length, 128);
        assert_eq!(embedding_truncation(0).max_length, 1, "0 is clamped to 1");
    }

    #[test]
    fn mean_pool_averages_real_tokens_and_normalizes() {
        // hidden=2, seq=3, mask drops the last (pad) token. Mean of tokens 0,1 =
        // ([1,0]+[3,0])/2 = [2,0], which L2-normalizes to [1,0].
        let data = [1.0, 0.0, 3.0, 0.0, 99.0, 99.0];
        let out = pool(&data, 2, 3, &[1, 1, 0], EmbeddingPooling::Mean);
        assert!((out[0] - 1.0).abs() < 1e-6, "got {out:?}");
        assert!(out[1].abs() < 1e-6, "got {out:?}");
    }

    #[test]
    fn cls_pool_takes_token_zero() {
        // CLS pooling ignores the mask and later tokens: take token 0 = [3,4],
        // which normalizes to [0.6, 0.8].
        let data = [3.0, 4.0, 100.0, 100.0];
        let out = pool(&data, 2, 2, &[1, 1], EmbeddingPooling::Cls);
        assert!((out[0] - 0.6).abs() < 1e-6, "got {out:?}");
        assert!((out[1] - 0.8).abs() < 1e-6, "got {out:?}");
    }

    #[test]
    fn l2_normalize_leaves_zero_vector_unchanged() {
        let mut z = vec![0.0, 0.0, 0.0];
        l2_normalize(&mut z);
        assert_eq!(z, vec![0.0, 0.0, 0.0]);
    }
}
