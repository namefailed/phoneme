//! Optional approximate-nearest-neighbour (ANN) vector index for semantic
//! search, backed by [usearch](https://github.com/unum-cloud/usearch) HNSW.
//!
//! This is the **only** module that names `usearch`, and every line that does is
//! behind `#[cfg(feature = "ann-usearch")]`. The default build compiles the
//! `#[cfg(not(feature = "ann-usearch"))]` stub instead — a type that is never
//! constructed — so the crate builds with zero new native code and the
//! brute-force cosine scan in [`super`] is the literal default. Confining all
//! FFI here keeps the rest of the catalog safe Rust (MEMORY flags an
//! un-audited unsafe-FFI gap — do not widen it past this module).
//!
//! ## How it plugs in
//!
//! The index maps a stable `u64` key (allocated by the `ann_keys` table) to one
//! chunk vector. A query asks usearch for `k = limit * oversample` nearest keys;
//! the parent module's `Catalog::vector_ranking` resolves those keys
//! back to recordings and re-scores the candidates with the **unchanged** cosine
//! / meeting-dedupe / fusion path. So the index only narrows *which* vectors are
//! scored, never *how* — displayed scores stay bit-identical to brute force and
//! the worst ANN can do is miss a tail result (tunable via `oversample` /
//! `expansion_search`).
//!
//! ## Fallback & persistence
//!
//! The sidecar file (`catalog.ann` next to `catalog.db`) is a **disposable
//! derived cache**; the f32 BLOBs in `embedding_chunks` are the only source of
//! truth. On any integrity doubt — missing file, usearch format/version skew,
//! dimension mismatch, or `ann_keys` count drift — the caller drops the sidecar
//! and rebuilds from SQLite, and any index error at query time falls through to
//! the brute-force scan. None of this can surface an error to the user.

#[cfg(feature = "ann-usearch")]
mod imp {
    use crate::config::AnnConfig;
    use crate::error::{Error, Result};
    use std::path::{Path, PathBuf};
    use usearch::ffi::{IndexOptions, MetricKind, ScalarKind};
    use usearch::Index;

    /// A live usearch HNSW index plus the on-disk sidecar it persists to.
    ///
    /// Keys are the `u64` ids from the `ann_keys` table; values are the chunk
    /// vectors. `dim` is the embedding dimension the index was built for — a
    /// query of a different length (the model was swapped) is rejected so we fall
    /// back to brute force rather than search a mismatched space.
    pub struct AnnIndex {
        index: Index,
        sidecar: PathBuf,
        dim: usize,
    }

    impl std::fmt::Debug for AnnIndex {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("AnnIndex")
                .field("sidecar", &self.sidecar)
                .field("dim", &self.dim)
                .field("size", &self.index.size())
                .finish()
        }
    }

    /// Map a usearch `cxx::Exception` to the crate error type. The C++ core's
    /// messages are opaque but stable enough to log; the caller treats any such
    /// error as "drop the sidecar and fall back to brute force".
    fn ann_err(context: &str, e: impl std::fmt::Display) -> Error {
        Error::Internal(format!("ann index: {context}: {e}"))
    }

    /// Build the usearch [`IndexOptions`] for a given dimension and config. f16
    /// quantization halves the index footprint; because candidate selection is
    /// followed by an exact f32 re-score, the lower precision only affects which
    /// candidates are *picked*, never a displayed score.
    fn options_for(dim: usize, cfg: &AnnConfig) -> IndexOptions {
        IndexOptions {
            dimensions: dim,
            // Vectors are L2-normalized at store time, so cosine == inner
            // product; cosine is used for clarity.
            metric: MetricKind::Cos,
            quantization: ScalarKind::F16,
            connectivity: cfg.connectivity,
            expansion_add: cfg.expansion_add,
            expansion_search: cfg.expansion_search,
            // One key → one vector. Re-embed removes the old keys explicitly, so
            // multi-vectors-per-key is neither needed nor wanted.
            multi: false,
        }
    }

    impl AnnIndex {
        /// The embedding dimension this index was built for.
        pub fn dim(&self) -> usize {
            self.dim
        }

        /// Number of vectors currently in the index.
        pub fn len(&self) -> usize {
            self.index.size()
        }

        /// Whether the index holds no vectors.
        pub fn is_empty(&self) -> bool {
            self.index.size() == 0
        }

        /// Create an empty index sized for `dim`, reserving room for
        /// `expected_capacity` vectors (it grows as needed).
        pub fn create(
            sidecar: PathBuf,
            dim: usize,
            expected_capacity: usize,
            cfg: &AnnConfig,
        ) -> Result<Self> {
            let index =
                Index::new(&options_for(dim, cfg)).map_err(|e| ann_err("create failed", e))?;
            // Reserve up front so a bulk build doesn't repeatedly reallocate the
            // graph; a zero reservation is fine for an incremental-only index.
            if expected_capacity > 0 {
                index
                    .reserve(expected_capacity)
                    .map_err(|e| ann_err("reserve failed", e))?;
            }
            Ok(Self {
                index,
                sidecar,
                dim,
            })
        }

        /// Build a fresh index from `(key, vector)` pairs, all sharing dimension
        /// `dim`. Vectors whose length doesn't match `dim` are skipped (the same
        /// guard the brute-force scan applies), so a stray bad blob can't fail
        /// the whole build.
        pub fn build_from_pairs(
            sidecar: PathBuf,
            dim: usize,
            pairs: &[(u64, Vec<f32>)],
            cfg: &AnnConfig,
        ) -> Result<Self> {
            let me = Self::create(sidecar, dim, pairs.len(), cfg)?;
            for (key, vec) in pairs {
                if vec.len() != dim {
                    continue;
                }
                me.index
                    .add(*key, vec)
                    .map_err(|e| ann_err("add during build failed", e))?;
            }
            Ok(me)
        }

        /// Load an index from its sidecar, verifying it matches `dim` and holds
        /// `expected_count` vectors. A mismatch (format skew after a crate bump,
        /// the model changed, or `ann_keys` drifted from the graph) is an error,
        /// so the caller rebuilds from SQLite — the sidecar is disposable.
        pub fn load_verified(
            sidecar: PathBuf,
            dim: usize,
            expected_count: usize,
            cfg: &AnnConfig,
        ) -> Result<Self> {
            if !sidecar.exists() {
                return Err(ann_err("load failed", "sidecar missing"));
            }
            let sidecar_str = sidecar
                .to_str()
                .ok_or_else(|| ann_err("load failed", "sidecar path is not valid utf-8"))?
                .to_string();
            let index = Index::new(&options_for(dim, cfg))
                .map_err(|e| ann_err("load: create failed", e))?;
            index
                .load(&sidecar_str)
                .map_err(|e| ann_err("load failed", e))?;
            if index.dimensions() != dim {
                return Err(ann_err(
                    "load failed",
                    format!(
                        "dimension mismatch: sidecar {} vs expected {dim}",
                        index.dimensions()
                    ),
                ));
            }
            if index.size() != expected_count {
                return Err(ann_err(
                    "load failed",
                    format!(
                        "count drift: sidecar {} vs SQLite {expected_count}",
                        index.size()
                    ),
                ));
            }
            Ok(Self {
                index,
                sidecar,
                dim,
            })
        }

        /// Insert one vector under `key`. A length mismatch is rejected rather
        /// than silently corrupting the space.
        pub fn add(&self, key: u64, vector: &[f32]) -> Result<()> {
            if vector.len() != self.dim {
                return Err(ann_err(
                    "add failed",
                    format!("dimension {} vs index {}", vector.len(), self.dim),
                ));
            }
            // usearch grows lazily, but reserve once past capacity to avoid a
            // realloc on every single insert during a long backfill.
            if self.index.size() >= self.index.capacity() {
                let want = (self.index.capacity().max(1)) * 2;
                self.index
                    .reserve(want)
                    .map_err(|e| ann_err("reserve-on-add failed", e))?;
            }
            self.index
                .add(key, vector)
                .map_err(|e| ann_err("add failed", e))
        }

        /// Remove a key from the index. usearch supports a real delete (not just
        /// a tombstone), so an actively edited library doesn't accumulate dead
        /// nodes. A key that isn't present is a no-op.
        pub fn remove(&self, key: u64) -> Result<()> {
            self.index
                .remove(key)
                .map(|_| ())
                .map_err(|e| ann_err("remove failed", e))
        }

        /// The `count` nearest keys to `query`, as `(key, distance)`. Distance is
        /// usearch's cosine distance; the caller ignores it and exact-re-scores
        /// the resolved vectors, so only the *set* of keys matters here.
        pub fn search(&self, query: &[f32], count: usize) -> Result<Vec<(u64, f32)>> {
            if query.len() != self.dim {
                return Err(ann_err(
                    "search failed",
                    format!("query dimension {} vs index {}", query.len(), self.dim),
                ));
            }
            let matches = self
                .index
                .search(query, count)
                .map_err(|e| ann_err("search failed", e))?;
            Ok(matches.keys.into_iter().zip(matches.distances).collect())
        }

        /// Persist the index to its sidecar. Called on graceful daemon shutdown,
        /// not per insert, to avoid an fsync per recording. The sidecar is
        /// disposable: if it's missing or stale on the next start, the index is
        /// rebuilt from SQLite.
        pub fn save(&self) -> Result<()> {
            let sidecar_str = self
                .sidecar
                .to_str()
                .ok_or_else(|| ann_err("save failed", "sidecar path is not valid utf-8"))?;
            self.index
                .save(sidecar_str)
                .map_err(|e| ann_err("save failed", e))
        }

        /// The sidecar path this index persists to.
        pub fn sidecar(&self) -> &Path {
            &self.sidecar
        }
    }
}

#[cfg(not(feature = "ann-usearch"))]
mod imp {
    use crate::config::AnnConfig;
    use crate::error::Result;
    use std::path::{Path, PathBuf};

    /// Stub ANN index for builds without the `ann-usearch` feature.
    ///
    /// It is never constructed (the only constructors are feature-gated, and the
    /// catalog only ever holds `None` in this configuration), so its methods are
    /// unreachable. It exists purely so the rest of the catalog can name
    /// [`AnnIndex`] in a field type and method signatures without `cfg`-forking
    /// every call site. The `#[allow(dead_code)]` is deliberate: this is dead by
    /// design in the default build.
    #[derive(Debug)]
    #[allow(dead_code)]
    pub struct AnnIndex {
        sidecar: PathBuf,
        dim: usize,
    }

    // No-op stubs mirroring the feature-on impl's documented surface; the real
    // semantics live on that impl, so the stub methods don't repeat the docs.
    #[allow(dead_code, missing_docs)]
    impl AnnIndex {
        pub fn dim(&self) -> usize {
            self.dim
        }
        pub fn len(&self) -> usize {
            0
        }
        pub fn is_empty(&self) -> bool {
            true
        }
        pub fn create(
            _sidecar: PathBuf,
            _dim: usize,
            _expected_capacity: usize,
            _cfg: &AnnConfig,
        ) -> Result<Self> {
            unreachable!("AnnIndex is unavailable without the ann-usearch feature")
        }
        pub fn build_from_pairs(
            _sidecar: PathBuf,
            _dim: usize,
            _pairs: &[(u64, Vec<f32>)],
            _cfg: &AnnConfig,
        ) -> Result<Self> {
            unreachable!("AnnIndex is unavailable without the ann-usearch feature")
        }
        pub fn load_verified(
            _sidecar: PathBuf,
            _dim: usize,
            _expected_count: usize,
            _cfg: &AnnConfig,
        ) -> Result<Self> {
            unreachable!("AnnIndex is unavailable without the ann-usearch feature")
        }
        pub fn add(&self, _key: u64, _vector: &[f32]) -> Result<()> {
            unreachable!("AnnIndex is unavailable without the ann-usearch feature")
        }
        pub fn remove(&self, _key: u64) -> Result<()> {
            unreachable!("AnnIndex is unavailable without the ann-usearch feature")
        }
        pub fn search(&self, _query: &[f32], _count: usize) -> Result<Vec<(u64, f32)>> {
            unreachable!("AnnIndex is unavailable without the ann-usearch feature")
        }
        pub fn save(&self) -> Result<()> {
            unreachable!("AnnIndex is unavailable without the ann-usearch feature")
        }
        pub fn sidecar(&self) -> &Path {
            &self.sidecar
        }
    }
}

pub use imp::AnnIndex;

/// Whether the `ann-usearch` feature was compiled in. Used by the Doctor probe
/// so it can report "disabled (not compiled)" vs "disabled (flag off)" without
/// the rest of the catalog `cfg`-forking.
pub const fn feature_compiled() -> bool {
    cfg!(feature = "ann-usearch")
}
