//! The recordings catalog — the durable home of the archive.
//!
//! This module owns [`Catalog`], the SQLite database every recording lands in.
//! The daemon writes to it as a recording moves through the pipeline (insert →
//! transcript → segments → summary/title → status), and the GUI/CLI read from it
//! for everything they show: the library list, the detail view, search, tags,
//! speaker names, and retention sweeps.
//!
//! A few conventions run through the whole file:
//!
//! - **Status is a string column.** [`RecordingStatus`] round-trips through
//!   stable lowercase strings (`"transcribing"`, `"hook_failed"`, …) via
//!   `parse_status`/`as_str`; a status the parser doesn't know errors the whole
//!   query, so every variant must have an arm.
//! - **Machine truth vs. user edits.** `original_transcript` (raw ASR) and
//!   `clean_transcript` (pipeline output) are preserved so a hand edit to the
//!   live `transcript` is reversible, and segments are stored separately and
//!   replaced wholesale on every (re)transcribe — user edits never rewrite them.
//! - **Search is hybrid.** Lexical FTS5 and per-chunk vector cosine are computed
//!   separately, fused with RRF ([`crate::fusion`]), and de-duplicated on a
//!   meeting-stable key so a meeting's two tracks collapse to one result.
//! - **WAL with bounded growth.** The pool runs in WAL mode; [`Catalog::open`]
//!   caps the WAL size and the daemon calls [`Catalog::checkpoint`] on idle so a
//!   long-lived reader can't let it grow without bound.

use crate::error::Result;
use crate::id::RecordingId;
use crate::tags::Tag;
use crate::types::{
    AiActivityEntry, ListFilter, NamedVoice, PropagationCandidate, Recording, RecordingStatus,
    SavedSearch, SpeakerName, SpeakerSuggestion, TranscriptSegment, TranscriptWord,
};
use chrono::{DateTime, Local};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// One stored embedding vector, deserialized once and held in memory: the
/// recording it belongs to, its meeting (for the meeting-stable dedupe key), and
/// the L2-normalized vector itself. Both the chunk path (`embedding_chunks`) and
/// the legacy whole-recording path (`embeddings`) produce these.
#[derive(Debug, Clone)]
struct CachedVector {
    /// Recording id this vector belongs to, as stored (the row's text id).
    id: String,
    /// The recording's `meeting_id`, if it is a meeting track — the dedupe key.
    meeting_id: Option<String>,
    /// The deserialized, L2-normalized embedding. `None` marks a blob that
    /// failed the 4-byte-alignment guard at load time, so the ranking paths can
    /// skip it without re-reading SQLite (preserving the warn-and-skip behavior).
    vector: Option<Vec<f32>>,
}

/// The deserialized embedding corpus, cached in memory so a search doesn't
/// re-read and re-decode every BLOB from SQLite on every query.
///
/// Two flat lists mirror the two SQL queries the ranking paths used to run on
/// each search: the per-chunk vectors (the primary, high-recall path) and the
/// legacy whole-recording vectors (the backfill fallback). The ranking code
/// keeps its own dimension/precedence logic; the corpus only removes the disk
/// read + decode, never changes which vectors are considered.
#[derive(Debug, Clone, Default)]
struct EmbeddingCorpus {
    /// Every row of `embedding_chunks`, decoded once.
    chunks: Vec<CachedVector>,
    /// Every row of `embeddings` (legacy whole-recording), decoded once.
    legacy: Vec<CachedVector>,
}

/// SQLite-backed recordings catalog.
///
/// All methods are async (Tokio). The pool is configured for WAL mode with
/// a small connection cap suitable for desktop usage (one writer at a time).
///
/// ## Embedding cache
///
/// Semantic search (`hybrid_search` → `vector_ranking`, `semantic_search`,
/// `more_like_this`) is a brute-force cosine scan over every stored vector. The
/// vectors live as little-endian f32 BLOBs in `embedding_chunks` / `embeddings`,
/// so each query used to `SELECT` and decode the *entire* corpus from disk — the
/// hot cost grows with the library and dominates a typed query / RAG turn.
///
/// `embedding_cache` holds the decoded corpus in memory so repeated queries
/// reuse it. The design is deliberately simple and pessimistic about staleness:
///
/// - **One whole-corpus snapshot**, not a per-id map. The ranking loops iterate
///   the full corpus anyway, so a snapshot mirrors them exactly and needs no
///   partial-miss reconciliation. It is rebuilt lazily on the first query after
///   any invalidation.
/// - **Coarse invalidation.** Every embedding write — `upsert_embedding`,
///   `upsert_chunk_embeddings`, `clear_all_embeddings` — and a recording
///   `delete` (which cascade-drops its embeddings) drops the snapshot. The next
///   query rebuilds it from SQLite. A stale vector returning a wrong ranking is
///   far worse than rebuilding, so invalidation never tries to be clever.
/// - **Bounded.** If the corpus exceeds a fixed vector-count cap it is *not*
///   cached; those (rare, very large) libraries fall back to reading from SQLite
///   each query, keeping memory bounded regardless of archive size.
/// - **Shared across clones.** The cache sits behind `Arc<RwLock<…>>`, so the
///   derived `Clone` (the daemon hands clones to its workers) shares one cache
///   and one set of invalidations rather than diverging per clone.
/// - **Lost-invalidation safe.** A miss snapshots a generation counter before it
///   reads from SQLite; an `invalidate_embedding_cache` racing between that read
///   and the store bumps the counter under the cache lock, so the store sees the
///   bump and declines to cache — the racing writer's view wins instead of being
///   clobbered by a snapshot taken before the write committed.
#[derive(Debug, Clone)]
pub struct Catalog {
    pool: SqlitePool,
    /// The decoded embedding corpus, or `None` when nothing is cached yet (cold,
    /// invalidated, or over the cache cap). Held behind an `Arc` so a warm hit
    /// returns the snapshot by cloning the `Arc` (O(1)) instead of deep-copying
    /// every vector. Shared across clones; see the type-level "Embedding cache"
    /// notes.
    embedding_cache: Arc<RwLock<Option<Arc<EmbeddingCorpus>>>>,
    /// Monotonic generation bumped on every embedding-cache invalidation. A
    /// corpus rebuild snapshots this before its SQL reads and, under the cache
    /// write lock at store time, only caches the snapshot when the generation is
    /// unchanged — so an invalidation that races the rebuild can't be lost. See
    /// the "Lost-invalidation safe" note above.
    embedding_cache_gen: Arc<AtomicU64>,
}

/// Upper bound on how many vectors the in-memory embedding cache will hold.
///
/// A 384-dim MiniLM vector is ~1.5 KB decoded; 200k vectors is ~300 MB, a
/// generous ceiling for a desktop archive (tens of chunks per recording ⇒ on the
/// order of 5–10k recordings). Above this the corpus is left uncached and the
/// ranking paths read from SQLite per query — slower, but memory stays bounded
/// no matter how large the library grows.
const MAX_CACHED_VECTORS: usize = 200_000;

/// How many AI-activity sessions to keep. The log is a recent-history audit
/// trail, not an archive — every insert prunes everything past this newest
/// window so the table stays bounded no matter how much the AI runs.
const AI_ACTIVITY_KEEP: i64 = 1_000;

/// Per-field char cap on a stored AI-activity `prompt`/`response`. Row count is
/// already bounded by `AI_ACTIVITY_KEEP`, but each prompt embeds the whole
/// transcript, so 1 000 long-meeting rows could still grow the table by
/// hundreds of MB. This ceiling sits far above any normal prompt or response —
/// so the 🧠 popout redisplays them verbatim — and only an extreme outlier is
/// truncated (with a marker) rather than stored in full.
const AI_ACTIVITY_FIELD_MAX_CHARS: usize = 64 * 1024;

/// Sanitizes a user-provided string for use in an FTS5 MATCH query.
///
/// Extracts alphanumeric terms and joins them with `* AND ` to perform a robust
/// prefix search that won't crash SQLite on invalid syntax.
fn sanitize_fts5_query(query: &str) -> String {
    let mut terms = Vec::new();
    let mut current_term = String::new();

    for c in query.chars() {
        if c.is_alphanumeric() {
            current_term.push(c);
        } else if !current_term.is_empty() {
            terms.push(format!("{}*", current_term));
            current_term.clear();
        }
    }

    if !current_term.is_empty() {
        terms.push(format!("{}*", current_term));
    }

    terms.join(" AND ")
}

/// The result of a [`Catalog::find_replace_transcript`] (S6): how many
/// occurrences were replaced and the resulting transcript. On a no-match,
/// `replaced` is 0 and `transcript` is the unchanged text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindReplaceOutcome {
    /// Number of occurrences replaced (0 = no-op, nothing written).
    pub replaced: usize,
    /// The transcript after the replacement (unchanged on a no-match).
    pub transcript: String,
}

/// One step's transcript output in a compounding recipe (PB-COMPOUND). `idx` is
/// the step order — `0` is the raw ASR, later rows are each Transform step's
/// output, and the last is the transcript that landed. Powers the Compare-versions
/// chain + revert. Stored via [`Catalog::replace_transcript_versions`] (wholesale
/// per (re)transcription, like segments) and read back in `idx` order.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TranscriptVersion {
    /// Step order; `0` = raw ASR, then one per Transform step.
    pub idx: i64,
    /// Recipe step id that produced it (e.g. `"cleanup"`); `None` for the raw row.
    pub step_id: Option<String>,
    /// Human label for the Compare-versions list (e.g. `"Cleanup (llama3.2)"`).
    pub label: Option<String>,
    /// Model that produced this version, if any.
    pub model: Option<String>,
    /// The transcript text at this step.
    pub text: String,
}

/// Table holding a recording's segment timeline for a timing variant (TL-CONSISTENCY):
/// `"cleaned"` → the post-cleanup re-aligned `transcript_segments_clean`, anything
/// else → the raw machine-truth `transcript_segments`. The returned name is a fixed
/// literal (never user input), so it is safe to interpolate into a query string.
fn segments_table(variant: &str) -> &'static str {
    if variant == "cleaned" {
        "transcript_segments_clean"
    } else {
        "transcript_segments"
    }
}

/// Word-level twin of [`segments_table`].
fn words_table(variant: &str) -> &'static str {
    if variant == "cleaned" {
        "transcript_words_clean"
    } else {
        "transcript_words"
    }
}

/// Case-insensitive literal find-replace over `haystack`: every run that equals
/// `needle` ignoring ASCII/Unicode case is replaced with `replacement` verbatim.
/// Returns `(count, new_string)`. Matching is by lowercased comparison; the
/// substituted text is always the caller's `replacement` (the matched run's
/// original casing is not preserved). `needle` is assumed non-empty (the caller
/// guards the empty case as a no-op).
///
/// Implementation note: matching walks the LOWERCASED haystack but copies from
/// the ORIGINAL, advancing by the original byte length of each matched run.
/// `char::to_lowercase` can change a string's length (e.g. some locale-specific
/// folds), so we compare per-`char`-boundary on the original rather than slicing
/// the lowercased copy back onto original byte offsets — keeping byte offsets
/// valid and the un-matched text byte-for-byte intact.
fn replace_ignore_case(haystack: &str, needle: &str, replacement: &str) -> (usize, String) {
    let needle_lower = needle.to_lowercase();
    let mut out = String::with_capacity(haystack.len());
    let mut count = 0usize;
    let mut rest = haystack;
    loop {
        // Find the next case-insensitive match start by scanning char boundaries
        // of `rest` and lowercasing the candidate window for comparison.
        let mut matched_at: Option<(usize, usize)> = None; // (start_byte, end_byte) in `rest`
        for (start, _) in rest.char_indices() {
            let window = &rest[start..];
            if window.to_lowercase().starts_with(&needle_lower) {
                // Recover the original-byte length of the matched run: take chars
                // from `window` until the lowercased prefix covers `needle_lower`.
                let mut consumed = 0usize;
                let mut low_len = 0usize;
                for ch in window.chars() {
                    consumed += ch.len_utf8();
                    low_len += ch.to_lowercase().map(|c| c.len_utf8()).sum::<usize>();
                    if low_len >= needle_lower.len() {
                        break;
                    }
                }
                matched_at = Some((start, start + consumed));
                break;
            }
        }
        match matched_at {
            Some((start, end)) => {
                out.push_str(&rest[..start]);
                out.push_str(replacement);
                count += 1;
                rest = &rest[end..];
            }
            None => {
                out.push_str(rest);
                break;
            }
        }
    }
    (count, out)
}

impl Catalog {
    /// Open (or create) a catalog database at `path`. Runs pending migrations.
    ///
    /// WAL configuration notes:
    /// - `journal_mode=WAL` + `synchronous=NORMAL` → ACID with crash safety,
    ///   no fsync per write.
    /// - `wal_autocheckpoint=1000` triggers an automatic checkpoint when the
    ///   WAL reaches ~1000 pages (~4 MB). Long-lived readers can still defer
    ///   the checkpoint, so `Catalog::checkpoint()` is called explicitly from
    ///   the daemon on idle to keep WAL growth bounded.
    /// - `journal_size_limit=67108864` caps the WAL at 64 MB regardless.
    /// - `busy_timeout=5s` makes a connection wait for a contended lock rather
    ///   than failing immediately with SQLITE_BUSY ("database is locked").
    pub async fn open(path: &Path) -> Result<Self> {
        let path_str = path.to_str().ok_or_else(|| {
            crate::error::Error::Internal("catalog path is not valid utf-8".into())
        })?;

        let opts = SqliteConnectOptions::from_str(path_str)?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            .foreign_keys(true)
            // Without a busy timeout SQLite returns SQLITE_BUSY ("database is
            // locked") the instant a connection can't take the lock. With a
            // small pool (max_connections below) a reader and the daemon's
            // writer can briefly contend, so wait up to 5s for the lock instead
            // of failing the whole query immediately.
            .busy_timeout(std::time::Duration::from_secs(5))
            .pragma("wal_autocheckpoint", "1000")
            .pragma("journal_size_limit", "67108864");

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self {
            pool,
            embedding_cache: Arc::new(RwLock::new(None)),
            embedding_cache_gen: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Drop the in-memory embedding snapshot. Called from every path that
    /// mutates a stored vector so the next search rebuilds from SQLite. A
    /// poisoned lock is recovered (cleared) rather than propagated: failing to
    /// invalidate is the dangerous outcome (stale rankings), so we always clear.
    ///
    /// The generation counter is bumped *while holding the cache write lock* so
    /// it is ordered with respect to a racing `embedding_corpus` store: that
    /// store snapshots the generation before its SQL reads and re-checks it under
    /// this same lock before caching, so an invalidation that lands between the
    /// snapshot's read and its store is observed (the bump) and the store backs
    /// off — the writer's invalidation can't be silently clobbered.
    fn invalidate_embedding_cache(&self) {
        match self.embedding_cache.write() {
            Ok(mut guard) => {
                self.embedding_cache_gen.fetch_add(1, Ordering::Release);
                *guard = None;
            }
            Err(poisoned) => {
                self.embedding_cache_gen.fetch_add(1, Ordering::Release);
                *poisoned.into_inner() = None;
            }
        }
    }

    /// The decoded embedding corpus, loaded once and held until the next write
    /// invalidates it.
    ///
    /// On a hit, returns the cached snapshot by cloning its `Arc` (O(1) — no deep
    /// copy of the vectors). The ranking loops consume the corpus by reference,
    /// so the shared `Arc<EmbeddingCorpus>` serves them all without copying. On a
    /// miss, reads both embedding tables, decodes every BLOB once, and caches the
    /// result unless it exceeds [`MAX_CACHED_VECTORS`] (in which case the corpus
    /// is returned but not stored, bounding memory) — or unless an invalidation
    /// raced the rebuild, in which case the freshly-read snapshot is returned but
    /// the slot is left cold so the racing writer's view wins (see the generation
    /// guard below).
    async fn embedding_corpus(&self) -> Result<Arc<EmbeddingCorpus>> {
        // Fast path: a warm snapshot. Read lock only; clone the Arc (O(1)).
        if let Ok(guard) = self.embedding_cache.read() {
            if let Some(corpus) = guard.as_ref() {
                return Ok(corpus.clone());
            }
        }

        // Snapshot the generation BEFORE the SQL reads. If an invalidation runs
        // while we read+decode below, it bumps this counter (under the cache
        // write lock), and the store step sees the mismatch and declines to
        // cache — so a vector changed mid-rebuild can't be masked by a snapshot
        // taken before that change committed.
        let gen_at_miss = self.embedding_cache_gen.load(Ordering::Acquire);

        // Miss: rebuild from SQLite. Decode happens outside any lock.
        let chunk_rows = sqlx::query(
            "SELECT ec.recording_id AS id, ec.vector AS vector, r.meeting_id AS meeting_id \
             FROM embedding_chunks ec JOIN recordings r ON r.id = ec.recording_id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut chunks = Vec::with_capacity(chunk_rows.len());
        for row in chunk_rows {
            chunks.push(row_to_cached_vector(&row)?);
        }

        let legacy_rows = sqlx::query(
            "SELECT e.id AS id, e.vector AS vector, r.meeting_id AS meeting_id \
             FROM embeddings e JOIN recordings r ON r.id = e.id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut legacy = Vec::with_capacity(legacy_rows.len());
        for row in legacy_rows {
            legacy.push(row_to_cached_vector(&row)?);
        }

        let corpus = Arc::new(EmbeddingCorpus { chunks, legacy });
        self.store_corpus_if_current(corpus.clone(), gen_at_miss);
        Ok(corpus)
    }

    /// Cache `corpus` under the write lock, but ONLY when the generation still
    /// matches `gen_at_miss` (the value snapshotted before the rebuild's SQL
    /// reads) and the corpus is under [`MAX_CACHED_VECTORS`].
    ///
    /// This is the store half of the lost-invalidation guard: holding the write
    /// lock here orders the generation re-read against `invalidate_embedding_cache`
    /// (which bumps the generation under the same lock), so an invalidation that
    /// raced the rebuild is observed as a mismatch and the slot is left cold —
    /// the racing writer's view wins instead of being clobbered by a snapshot
    /// taken before its write committed.
    fn store_corpus_if_current(&self, corpus: Arc<EmbeddingCorpus>, gen_at_miss: u64) {
        // Large libraries stay uncached so memory can't grow without bound.
        if !Self::cap_allows_caching(corpus.chunks.len() + corpus.legacy.len()) {
            return;
        }
        let mut guard = match self.embedding_cache.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if self.embedding_cache_gen.load(Ordering::Acquire) == gen_at_miss {
            *guard = Some(corpus);
        }
    }

    /// Whether a corpus of `total_vectors` is small enough to cache in memory.
    /// The single source of truth for the [`MAX_CACHED_VECTORS`] bound, so the
    /// loader and the test that proves boundedness agree by construction.
    fn cap_allows_caching(total_vectors: usize) -> bool {
        total_vectors <= MAX_CACHED_VECTORS
    }

    /// Test-only view of the embedding cache: the number of vectors currently
    /// held in the warm snapshot, or `None` when the snapshot is cold (never
    /// loaded, invalidated, or skipped for being over the cap). Lets the cache
    /// tests assert warm/cold state and the bound without exposing internals.
    #[cfg(test)]
    fn cached_vector_count(&self) -> Option<usize> {
        self.embedding_cache
            .read()
            .unwrap()
            .as_ref()
            .map(|c| c.chunks.len() + c.legacy.len())
    }

    /// Persist one completed AI-activity session (a finished streaming LLM
    /// stage). Called by the daemon's `run_llm_stage` on success so the 🧠 popout
    /// can show the prompt + response after an app restart, not just live. Every
    /// insert prunes the table back to the newest `AI_ACTIVITY_KEEP` rows so it
    /// can't grow without bound; `created_at` is stored as RFC3339 UTC.
    pub async fn insert_ai_activity(
        &self,
        recording_id: &str,
        stage: &str,
        prompt: &str,
        response: &str,
    ) -> Result<()> {
        // Cap each field so a pathologically long transcript can't bloat the row.
        // Truncate on a char boundary (these fields are UTF-8 transcript text) so
        // a multi-byte character is never split, and append a marker so the popout
        // shows the text was clipped rather than silently ending mid-word.
        let cap = |s: &str| -> String {
            if s.chars().count() <= AI_ACTIVITY_FIELD_MAX_CHARS {
                return s.to_string();
            }
            let end = s
                .char_indices()
                .nth(AI_ACTIVITY_FIELD_MAX_CHARS)
                .map(|(i, _)| i)
                .unwrap_or(s.len());
            format!("{}… [truncated]", &s[..end])
        };
        let prompt = cap(prompt);
        let response = cap(response);
        sqlx::query(
            "INSERT INTO ai_activity (recording_id, stage, prompt, response, created_at) \
             VALUES (?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ','now'))",
        )
        .bind(recording_id)
        .bind(stage)
        .bind(&prompt)
        .bind(&response)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "DELETE FROM ai_activity WHERE id NOT IN \
             (SELECT id FROM ai_activity ORDER BY id DESC LIMIT ?)",
        )
        .bind(AI_ACTIVITY_KEEP)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Recent AI-activity sessions, newest first. With `recording_id` set, only
    /// that recording's sessions; otherwise the whole library's recent activity.
    /// `limit` is clamped to `[1, AI_ACTIVITY_KEEP]`.
    pub async fn list_ai_activity(
        &self,
        recording_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<AiActivityEntry>> {
        let limit = limit.clamp(1, AI_ACTIVITY_KEEP);
        let rows = match recording_id {
            Some(rid) => {
                sqlx::query(
                    "SELECT id, recording_id, stage, prompt, response, created_at \
                     FROM ai_activity WHERE recording_id = ? ORDER BY id DESC LIMIT ?",
                )
                .bind(rid)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT id, recording_id, stage, prompt, response, created_at \
                     FROM ai_activity ORDER BY id DESC LIMIT ?",
                )
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(AiActivityEntry {
                id: row.try_get("id")?,
                recording_id: row.try_get("recording_id")?,
                stage: row.try_get("stage")?,
                prompt: row.try_get("prompt")?,
                response: row.try_get("response")?,
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(out)
    }

    /// All saved searches, most-recently-updated first. The `filter_json` is
    /// returned verbatim for the frontend to deserialize.
    pub async fn list_saved_searches(&self) -> Result<Vec<SavedSearch>> {
        let rows = sqlx::query(
            "SELECT id, name, filter_json FROM saved_searches \
             ORDER BY updated_at DESC, name COLLATE NOCASE ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(SavedSearch {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                filter_json: row.try_get("filter_json")?,
            });
        }
        Ok(out)
    }

    /// Insert or update a saved search by id. The frontend owns the by-name
    /// upsert and rename-conflict rules (it picks the id to write), so this is a
    /// plain by-id upsert — `created_at` is set once, `updated_at` on every write.
    pub async fn upsert_saved_search(&self, id: &str, name: &str, filter_json: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO saved_searches (id, name, filter_json, created_at, updated_at) \
             VALUES (?, ?, ?, datetime('now'), datetime('now')) \
             ON CONFLICT(id) DO UPDATE SET \
                 name = excluded.name, \
                 filter_json = excluded.filter_json, \
                 updated_at = datetime('now')",
        )
        .bind(id)
        .bind(name)
        .bind(filter_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete a saved search by id (unknown ids are a no-op). Returns whether a
    /// row was actually removed.
    pub async fn delete_saved_search(&self, id: &str) -> Result<bool> {
        let res = sqlx::query("DELETE FROM saved_searches WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Execute a stored saved search by id, server-side: look up its
    /// `filter_json`, parse it into a [`ListFilter`], and run the normal
    /// [`Self::list`] query — so a saved search produces the *same* recordings
    /// shape as a plain list without the frontend re-deriving the filter (S2).
    ///
    /// The persisted filter is the frontend's `UiFilter`
    /// ([`crate::SavedSearchFilter`]); the four-way `kind` and `tag_state` are
    /// mapped onto the daemon's `kind`/`favorite`/`in_place`/`tagged` exactly as
    /// the frontend's `toWireFilter` does, and UI-only display state (semantic /
    /// like-mode) is ignored — executing a saved search runs the *list* query,
    /// not a similarity/semantic search.
    ///
    /// Errors: [`crate::Error::NotFound`] for an unknown id, and
    /// [`crate::Error::InvalidConfig`] when the stored `filter_json` won't parse
    /// (a hand-edit, a stale shape) — surfaced to the client verbatim rather than
    /// silently running the whole library.
    pub async fn run_saved_search(&self, id: &str) -> Result<Vec<Recording>> {
        let row = sqlx::query("SELECT filter_json FROM saved_searches WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Err(crate::error::Error::NotFound {
                id: format!("saved search {id}"),
            });
        };
        let filter_json: String = row.try_get("filter_json")?;
        let filter = crate::SavedSearchFilter::parse_to_list_filter(&filter_json)?;
        self.list(&filter).await
    }

    /// Insert a new recording row. The pipeline calls this once, when capture
    /// starts; later stages update the same row in place.
    pub async fn insert(&self, r: &Recording) -> Result<()> {
        sqlx::query(
            "INSERT INTO recordings (
                 id, started_at, duration_ms, audio_path, transcript, model, status,
                 error_kind, error_message, hook_command, hook_exit_code, hook_duration_ms,
                 transcribed_at, hook_ran_at, notes, meeting_id, meeting_name, track, in_place,
                 cleanup_model, diarized
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(r.id.as_str())
        .bind(r.started_at.to_rfc3339())
        .bind(r.duration_ms)
        .bind(&r.audio_path)
        .bind(r.transcript.as_deref())
        .bind(r.model.as_deref())
        .bind(r.status.as_str())
        .bind(r.error_kind.as_deref())
        .bind(r.error_message.as_deref())
        .bind(r.hook_command.as_deref())
        .bind(r.hook_exit_code)
        .bind(r.hook_duration_ms)
        .bind(r.transcribed_at.map(|d| d.to_rfc3339()))
        .bind(r.hook_ran_at.map(|d| d.to_rfc3339()))
        .bind(r.notes.as_deref())
        .bind(r.meeting_id.as_deref())
        .bind(r.meeting_name.as_deref())
        .bind(r.track.as_deref())
        .bind(r.in_place)
        .bind(r.cleanup_model.as_deref())
        .bind(r.diarized)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Insert a recording with EVERY persisted DTO column at once — the faithful
    /// inverse of the library backup export ([`crate::backup`]).
    ///
    /// The pipeline's [`Catalog::insert`] writes only the columns a fresh
    /// recording starts with and fills the rest (title, summary, favorite, the
    /// per-step model names, …) later via dedicated setters as it advances. A
    /// backup restore has all of those values up front and must land them in one
    /// row, so this writes the full column set. Fields the DTO does NOT carry —
    /// `original_transcript` / `clean_transcript`, segments, words, embeddings,
    /// voiceprints — are bounded by what the export captured and are simply not
    /// restored. `INSERT` (not upsert): the restore caller skips ids that already
    /// exist, so a re-import never overwrites a row (idempotent), and a genuine
    /// id clash surfaces as an error rather than silently clobbering.
    pub async fn insert_restored(&self, r: &Recording) -> Result<()> {
        let tag_suggestions = if r.tag_suggestions.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&r.tag_suggestions)?)
        };
        sqlx::query(
            "INSERT INTO recordings (
                 id, started_at, duration_ms, audio_path, transcript, model, status,
                 error_kind, error_message, hook_command, hook_exit_code, hook_duration_ms,
                 transcribed_at, hook_ran_at, notes, meeting_id, meeting_name, track, in_place,
                 cleanup_model, diarized, user_edited, favorite, tag_suggestions, summary,
                 summary_model, title, title_is_auto, title_model, tag_model, diarization_model
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(r.id.as_str())
        .bind(r.started_at.to_rfc3339())
        .bind(r.duration_ms)
        .bind(&r.audio_path)
        .bind(r.transcript.as_deref())
        .bind(r.model.as_deref())
        .bind(r.status.as_str())
        .bind(r.error_kind.as_deref())
        .bind(r.error_message.as_deref())
        .bind(r.hook_command.as_deref())
        .bind(r.hook_exit_code)
        .bind(r.hook_duration_ms)
        .bind(r.transcribed_at.map(|d| d.to_rfc3339()))
        .bind(r.hook_ran_at.map(|d| d.to_rfc3339()))
        .bind(r.notes.as_deref())
        .bind(r.meeting_id.as_deref())
        .bind(r.meeting_name.as_deref())
        .bind(r.track.as_deref())
        .bind(r.in_place)
        .bind(r.cleanup_model.as_deref())
        .bind(r.diarized)
        .bind(r.user_edited)
        .bind(r.favorite)
        .bind(tag_suggestions)
        .bind(r.summary.as_deref())
        .bind(r.summary_model.as_deref())
        .bind(r.title.as_deref())
        .bind(r.title_is_auto)
        .bind(r.title_model.as_deref())
        .bind(r.tag_model.as_deref())
        .bind(r.diarization_model.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set (or clear) the display name for every track sharing `meeting_id`.
    pub async fn update_meeting_name(&self, meeting_id: &str, name: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE recordings SET meeting_name = ?, updated_at = datetime('now') WHERE meeting_id = ?")
            .bind(name)
            .bind(meeting_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Move a recording to a new lifecycle status (the pipeline calls this as it
    /// advances through transcribing → cleaning up → … → done/failed).
    pub async fn update_status(&self, id: &RecordingId, status: RecordingStatus) -> Result<()> {
        sqlx::query("UPDATE recordings SET status = ?, updated_at = datetime('now') WHERE id = ?")
            .bind(status.as_str())
            .bind(id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record why a recording failed, on the row itself.
    ///
    /// `kind` is the short machine label the failed path already uses for the
    /// inbox quarantine (e.g. `"whisper_error"`, `"hook_failed"`) and `message`
    /// is the human-readable reason. Storing them here makes the failure reason
    /// survive a daemon restart: the live failure events and the `failed/`
    /// quarantine JSON are otherwise the only places it lives, and neither is
    /// readable once the app session that saw the event is gone. The status
    /// itself is set separately by [`Self::update_status`]; this only fills the
    /// two error columns.
    pub async fn update_error(&self, id: &RecordingId, kind: &str, message: &str) -> Result<()> {
        sqlx::query(
            "UPDATE recordings SET error_kind = ?, error_message = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(kind)
        .bind(message)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update both status and duration in a single query.
    pub async fn update_status_and_duration(
        &self,
        id: &RecordingId,
        status: RecordingStatus,
        duration_ms: i64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE recordings SET status = ?, duration_ms = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(duration_ms)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update the transcript after machine transcription.
    ///
    /// `transcript` is the text to store as the live transcript — for
    /// recordings with LLM post-processing enabled this will be the LLM-
    /// cleaned text. `original_transcript` is **always** the raw Whisper
    /// output, so the "View original" feature shows the pre-LLM version even
    /// when post-processing is active. Re-transcription overwrites both
    /// columns (fresh baseline) and clears any stored failure reason
    /// (`error_kind`/`error_message`), so a successful retry of a previously
    /// failed recording doesn't keep showing the old error.
    pub async fn update_transcript(
        &self,
        id: &RecordingId,
        transcript: &str,
        original_transcript: &str,
        model: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET transcript = ?, original_transcript = ?, clean_transcript = ?, model = ?,
                   user_edited = 0, error_kind = NULL, error_message = NULL,
                   transcribed_at = datetime('now'), updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(transcript)
        .bind(original_transcript)
        // `clean_transcript` snapshots the pipeline output (transcribed + cleaned)
        // so "View unedited transcript" can show it even after the user edits the
        // live transcript. User edits go through `update_user_transcript`, which
        // leaves this column untouched.
        .bind(transcript)
        .bind(model)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record which post-processing LLM model ran (if any), whether speaker
    /// diarization was applied, and the diarizer's model when a cloud diarizer
    /// produced it (`None` for the local speakrs diarizer or none at all).
    /// Called by the pipeline after transcription so the list view and the
    /// detail provenance line can surface these.
    pub async fn update_processing_meta(
        &self,
        id: &RecordingId,
        cleanup_model: Option<&str>,
        diarized: bool,
        diarization_model: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET cleanup_model = ?, diarized = ?, diarization_model = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(cleanup_model)
        .bind(diarized)
        .bind(diarization_model)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record the LLM model the auto-tagger used for this recording (the detail
    /// provenance line names it). Written once per auto-tag run, independent of
    /// whether the run produced approve/dismiss suggestions or auto-accepted
    /// existing tags — so the step shows even when nothing was left to approve.
    pub async fn set_tag_model(&self, id: &RecordingId, model: &str) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET tag_model = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(model)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Store (or replace) the LLM-generated summary for a recording, along with
    /// the model that produced it.
    pub async fn update_summary(
        &self,
        id: &RecordingId,
        summary: &str,
        model: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET summary = ?, summary_model = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(summary)
        .bind(model)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update the transcript from a manual user edit, preserving
    /// `original_transcript`/`clean_transcript` so the edit can be reverted.
    /// Sets the `user_edited` flag and — crucially — leaves `model` untouched so
    /// the "Transcript Model" column keeps showing the transcription model that
    /// actually produced the text (the "Edited" column surfaces the hand edit).
    pub async fn update_user_transcript(&self, id: &RecordingId, transcript: &str) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET transcript = ?, user_edited = 1, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(transcript)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Find-and-replace across a recording's stored **live** transcript (S6).
    ///
    /// ### Semantics
    /// - **Literal** substring replacement, NOT regex — the safest default (no
    ///   accidental metacharacter surprises on a transcript). `find` is matched
    ///   verbatim; `replace` is inserted verbatim.
    /// - **Case sensitivity** is opt-out via `case_sensitive`. A case-insensitive
    ///   match still substitutes the user's `replace` text exactly (the original
    ///   casing of each matched run is not preserved — documented).
    /// - Returns the **count** of occurrences replaced.
    ///
    /// ### Scope
    /// Only the live `transcript` is rewritten — the same column hand edits and
    /// `update_user_transcript` touch. The preserved `original_transcript`
    /// (machine output) and `clean_transcript` (pipeline output) are deliberately
    /// left intact so the edit stays revertible; per-segment / per-word layers are
    /// re-flowed by the caller (the daemon) exactly as for a normal transcript
    /// edit. The recording is marked `user_edited`.
    ///
    /// ### No-op safety
    /// An empty `find`, or zero matches, performs **no write** and returns 0 — a
    /// no-match never rewrites (and so never corrupts) the transcript, and never
    /// touches `updated_at`/`user_edited`.
    ///
    /// Errors: [`crate::Error::NotFound`] when the id is unknown OR the recording
    /// has no transcript yet (nothing to edit).
    pub async fn find_replace_transcript(
        &self,
        id: &RecordingId,
        find: &str,
        replace: &str,
        case_sensitive: bool,
    ) -> Result<FindReplaceOutcome> {
        // Empty needle: defined as a no-op (replacing "" would otherwise splice
        // `replace` between every char). Resolve the id first so an unknown id
        // still reports NotFound rather than a silent 0.
        let Some(rec) = self.get(id).await? else {
            return Err(crate::error::Error::NotFound { id: id.to_string() });
        };
        let Some(current) = rec.transcript else {
            return Err(crate::error::Error::NotFound {
                id: format!("{id} (no transcript to edit)"),
            });
        };
        if find.is_empty() {
            return Ok(FindReplaceOutcome {
                replaced: 0,
                transcript: current,
            });
        }

        let (count, new_text) = if case_sensitive {
            (
                current.matches(find).count(),
                current.replace(find, replace),
            )
        } else {
            replace_ignore_case(&current, find, replace)
        };

        // No match → no write (no-op safety: never rewrite on zero matches).
        if count == 0 {
            return Ok(FindReplaceOutcome {
                replaced: 0,
                transcript: current,
            });
        }

        self.update_user_transcript(id, &new_text).await?;
        Ok(FindReplaceOutcome {
            replaced: count,
            transcript: new_text,
        })
    }

    /// Replace the LLM-suggested tags awaiting approval for a recording.
    /// An empty slice clears the column (no lingering empty-array JSON).
    pub async fn set_tag_suggestions(&self, id: &RecordingId, names: &[String]) -> Result<()> {
        let json = if names.is_empty() {
            None
        } else {
            Some(serde_json::to_string(names)?)
        };
        sqlx::query(
            r#"UPDATE recordings
               SET tag_suggestions = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(json)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Drop EVERY pending tag suggestion across the whole library (the
    /// Auto-Tagging settings' "Clear all suggestions" action). Returns how
    /// many recordings actually had suggestions to clear.
    pub async fn clear_all_tag_suggestions(&self) -> Result<u64> {
        let result = sqlx::query(
            r#"UPDATE recordings
               SET tag_suggestions = NULL, updated_at = datetime('now')
               WHERE tag_suggestions IS NOT NULL"#,
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Set or clear a recording's display title.
    ///
    /// Ownership rule: a user title (`title_is_auto = 0`) wins forever — an
    /// auto write (`is_auto = true` with a title) only lands while the title
    /// is still auto-owned, so the pipeline can refresh its own titles on
    /// retranscribe but can never clobber one the user typed. Explicit user
    /// writes always apply: `Some` + `is_auto = false` takes ownership;
    /// `None` clears the title AND reverts ownership to auto, so the next
    /// pipeline run generates a fresh one.
    ///
    /// Returns whether a row was actually updated (`false` = unknown id, or
    /// an auto write skipped because the user owns the title).
    ///
    /// `model` records which LLM produced an auto title for the provenance line:
    /// the auto-title step passes `Some(model)` when an LLM made the title and
    /// `None` for a heuristic one; user/CLI title writes pass `None`, which also
    /// clears any stale model so a user-owned title never shows one.
    pub async fn set_title(
        &self,
        id: &RecordingId,
        title: Option<&str>,
        is_auto: bool,
        model: Option<&str>,
    ) -> Result<bool> {
        // A cleared title is always auto-owned — `None` means "no title,
        // generate one next run", never "user-owned empty title".
        let is_auto = is_auto || title.is_none();
        let result = if is_auto && title.is_some() {
            sqlx::query(
                r#"UPDATE recordings
                   SET title = ?, title_is_auto = 1, title_model = ?, updated_at = datetime('now')
                   WHERE id = ? AND title_is_auto = 1"#,
            )
            .bind(title)
            .bind(model)
            .bind(id.as_str())
            .execute(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"UPDATE recordings
                   SET title = ?, title_is_auto = ?, title_model = ?, updated_at = datetime('now')
                   WHERE id = ?"#,
            )
            .bind(title)
            .bind(is_auto)
            .bind(model)
            .bind(id.as_str())
            .execute(&self.pool)
            .await?
        };
        Ok(result.rows_affected() > 0)
    }

    /// Set or clear the "favorite"/star flag for a recording (Favorites view).
    pub async fn set_favorite(&self, id: &RecordingId, favorite: bool) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET favorite = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(favorite as i64)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The preserved original (machine) transcript, if any. `None` for
    /// recordings transcribed before this column existed, or never transcribed.
    pub async fn get_original_transcript(&self, id: &RecordingId) -> Result<Option<String>> {
        let row = sqlx::query("SELECT original_transcript FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => Ok(r.try_get::<Option<String>, _>("original_transcript")?),
            None => Ok(None),
        }
    }

    /// The preserved "unedited" transcript — the pipeline output (machine
    /// transcription + any LLM cleanup) before the user made hand edits. `None`
    /// for recordings transcribed before this column existed, or never
    /// transcribed.
    pub async fn get_clean_transcript(&self, id: &RecordingId) -> Result<Option<String>> {
        let row = sqlx::query("SELECT clean_transcript FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => Ok(r.try_get::<Option<String>, _>("clean_transcript")?),
            None => Ok(None),
        }
    }

    /// Update the free-form user notes for a recording.
    ///
    /// Notes live in their own column and are completely independent of the
    /// transcript: neither machine (re-)transcription (`update_transcript`)
    /// nor user transcript edits (`update_user_transcript`) touch this column,
    /// so notes always survive those operations.
    pub async fn update_notes(&self, id: &RecordingId, notes: &str) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET notes = ?, updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(notes)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record the outcome of the hook that ran for a recording (command, exit
    /// code, duration), stamping `hook_ran_at`.
    pub async fn update_hook_result(
        &self,
        id: &RecordingId,
        command: &str,
        exit_code: i32,
        duration_ms: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE recordings
               SET hook_command = ?, hook_exit_code = ?, hook_duration_ms = ?,
                   hook_ran_at = datetime('now'), updated_at = datetime('now')
               WHERE id = ?"#,
        )
        .bind(command)
        .bind(exit_code)
        .bind(duration_ms)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The recording's Meeting-Mode track and meeting link — `(track,
    /// meeting_id)` — without the speaker-name join [`get`](Self::get) does.
    ///
    /// The pipeline needs only these two columns *before* transcribing to drive
    /// track-aware Meeting Mode (a meeting's mic track is labelled as one fixed
    /// speaker instead of diarized), so this narrow read keeps that off the hot
    /// path from paying for the full row + join. Both columns are `None` for a
    /// normal single-track recording; `(None, None)` when the id is unknown.
    pub async fn track_and_meeting(
        &self,
        id: &RecordingId,
    ) -> Result<(Option<String>, Option<String>)> {
        let row = sqlx::query("SELECT track, meeting_id FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => Ok((r.try_get("track")?, r.try_get("meeting_id")?)),
            None => Ok((None, None)),
        }
    }

    /// Fetch a single recording by id, with its custom speaker names populated
    /// (tags are loaded separately via [`Catalog::tags_for`]). `None` when the
    /// id is unknown.
    pub async fn get(&self, id: &RecordingId) -> Result<Option<Recording>> {
        let row = sqlx::query("SELECT * FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?;
        let mut rec = match row.map(row_to_recording).transpose()? {
            Some(r) => r,
            None => return Ok(None),
        };
        // Populate the speaker-name map so a single-recording fetch (the daemon's
        // GetRecording, which backs the detail view) can render custom names.
        // Tags are intentionally NOT loaded here — the detail view fetches those
        // separately via `tags_for`, matching prior behavior.
        rec.speaker_names = self.speaker_names_for(&rec.id).await.unwrap_or_default();
        Ok(Some(rec))
    }

    /// Upsert the semantic embedding vector for a recording.
    pub async fn upsert_embedding(&self, id: &RecordingId, vector: &[f32]) -> Result<()> {
        // Pack f32 array into little-endian bytes.
        let mut bytes = Vec::with_capacity(vector.len() * 4);
        for &v in vector {
            bytes.extend_from_slice(&v.to_le_bytes());
        }

        sqlx::query(
            "INSERT INTO embeddings (id, vector) VALUES (?, ?)
             ON CONFLICT(id) DO UPDATE SET vector = excluded.vector",
        )
        .bind(id.as_str())
        .bind(bytes)
        .execute(&self.pool)
        .await?;

        // A vector changed on disk — drop the in-memory snapshot so the next
        // search rebuilds it (a stale cached vector would rank wrongly).
        self.invalidate_embedding_cache();
        Ok(())
    }

    /// Recordings with a transcript but no legacy whole-recording embedding yet.
    /// Drives the embedding backfill for the older `embeddings` table.
    pub async fn list_recordings_without_embeddings(&self) -> Result<Vec<Recording>> {
        let rows = sqlx::query(
            "SELECT * FROM recordings \
             WHERE id NOT IN (SELECT id FROM embeddings) \
             AND transcript IS NOT NULL AND transcript != ''",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_recording).collect()
    }

    /// Replace all chunk embeddings for a recording in one transaction.
    ///
    /// Per-chunk embeddings (one vector per sentence-aware chunk) are what make
    /// paraphrase recall work on longer notes — see [`crate::chunk`]. Re-embedding
    /// deletes the recording's existing chunks first so a re-transcription or an
    /// edit can't leave stale vectors from the previous text behind. An empty
    /// `vectors` (e.g. a blank transcript) just clears the chunks.
    pub async fn upsert_chunk_embeddings(
        &self,
        id: &RecordingId,
        vectors: &[Vec<f32>],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM embedding_chunks WHERE recording_id = ?")
            .bind(id.as_str())
            .execute(&mut *tx)
            .await?;
        for (idx, vector) in vectors.iter().enumerate() {
            let mut bytes = Vec::with_capacity(vector.len() * 4);
            for &v in vector {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            sqlx::query(
                "INSERT INTO embedding_chunks (recording_id, chunk_index, vector) VALUES (?, ?, ?)",
            )
            .bind(id.as_str())
            .bind(idx as i64)
            .bind(bytes)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        // A recording's chunk vectors were replaced — drop the snapshot.
        self.invalidate_embedding_cache();
        Ok(())
    }

    /// Delete ALL stored embeddings — per-chunk and legacy whole-recording — so
    /// the whole library can be re-embedded with a newly-configured model. After
    /// this every recording counts as "without chunk embeddings", so the daemon's
    /// backfill re-embeds them. Vectors from a different model/dimension would
    /// otherwise be silently skipped (dimension guard) and the recording would be
    /// unsearchable until re-embedded.
    pub async fn clear_all_embeddings(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM embedding_chunks")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM embeddings")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        // Whole library wiped for a re-embed — drop the snapshot.
        self.invalidate_embedding_cache();
        Ok(())
    }

    /// Recordings that have a transcript but no chunk embeddings yet. Drives the
    /// daemon's one-time backfill that migrates the library from the legacy
    /// whole-recording `embeddings` table to per-chunk vectors.
    pub async fn list_recordings_without_chunk_embeddings(&self) -> Result<Vec<Recording>> {
        let rows = sqlx::query(
            "SELECT * FROM recordings \
             WHERE id NOT IN (SELECT DISTINCT recording_id FROM embedding_chunks) \
             AND transcript IS NOT NULL AND transcript != ''",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_recording).collect()
    }

    /// Loads all embeddings into memory for brute-force cosine similarity.
    pub async fn load_all_embeddings(&self) -> Result<Vec<(RecordingId, Vec<f32>)>> {
        let rows = sqlx::query("SELECT id, vector FROM embeddings")
            .fetch_all(&self.pool)
            .await?;

        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("id")?;
            let bytes: Vec<u8> = row.try_get("vector")?;

            if !bytes.len().is_multiple_of(4) {
                tracing::warn!(
                    "Embedding for {} has invalid byte length: {}",
                    id,
                    bytes.len()
                );
                continue;
            }

            let mut vec = Vec::with_capacity(bytes.len() / 4);
            for chunk in bytes.chunks_exact(4) {
                vec.push(f32::from_le_bytes(chunk.try_into().unwrap()));
            }

            if let Some(rec_id) = RecordingId::parse(id) {
                results.push((rec_id, vec));
            }
        }

        Ok(results)
    }

    /// Semantic search across embedded recordings, returning the top matches as
    /// `(id, cosine_score)` sorted high→low.
    ///
    /// - **Dimension safety:** an embedding whose length doesn't match the query
    ///   vector is skipped (cosine over mismatched dimensions is meaningless and
    ///   would otherwise score on a silently-truncated prefix).
    /// - **Relevance floor:** results scoring below `min_score` are dropped, so a
    ///   vague/garbage query returns *few or no* results instead of `limit`
    ///   arbitrary ones.
    /// - **Meeting dedupe:** a meeting's two tracks share a `meeting_id` and have
    ///   near-identical transcripts; they collapse to a single best-scoring entry
    ///   so they don't crowd out other recordings. Standalone recordings are keyed
    ///   by their own id.
    pub async fn semantic_search(
        &self,
        query_vec: &[f32],
        limit: usize,
        min_score: f32,
    ) -> Result<Vec<(RecordingId, f32)>> {
        // Reads the legacy whole-recording vectors from the shared decoded
        // corpus, so a query doesn't re-read and re-decode the BLOBs each time.
        let corpus = self.embedding_corpus().await?;

        let dim = query_vec.len();
        let query: Vec<f32> = query_vec.to_vec();

        // The cosine scan over the legacy corpus is CPU-bound — up to
        // MAX_CACHED_VECTORS dot products — so run it on the blocking pool. On a
        // large library / slow box, doing it inline would starve the async
        // executor (IPC named-pipe reads, audio streaming) between await points.
        let best = tokio::task::spawn_blocking(move || {
            // Best (id, score) per result key — meeting_id when present, else the
            // recording id — so a meeting contributes at most one result.
            let mut best: std::collections::HashMap<String, (RecordingId, f32)> =
                std::collections::HashMap::new();
            for cv in &corpus.legacy {
                let Some(vec) = cv.vector.as_deref() else {
                    continue; // corrupt blob, already warned at load
                };
                if vec.len() != dim {
                    tracing::warn!(id = %cv.id, dim = vec.len(), query_dim = dim, "skipping embedding: dimension mismatch");
                    continue;
                }

                let score = crate::embed::Embedder::cosine_similarity(&query, vec);
                if score < min_score {
                    continue;
                }
                let Some(rec_id) = RecordingId::parse(cv.id.clone()) else {
                    continue;
                };
                let key = cv.meeting_id.clone().unwrap_or_else(|| cv.id.clone());
                best.entry(key)
                    .and_modify(|e| {
                        if score > e.1 {
                            *e = (rec_id.clone(), score);
                        }
                    })
                    .or_insert((rec_id, score));
            }
            best
        })
        .await
        .map_err(|e| crate::error::Error::Internal(format!("semantic search task failed: {e}")))?;

        let mut scores: Vec<(RecordingId, f32)> = best.into_values().collect();
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(limit);
        Ok(scores)
    }

    /// Compute the per-recording **best-chunk** (max-sim) cosine ranking for a
    /// query vector, meeting-deduped.
    ///
    /// Returns `(dedupe_key, RecordingId, raw_cosine)` sorted high→low. The raw
    /// cosine is of the single best-matching chunk, which is what makes a
    /// paraphrase of one spoken idea rank on that idea instead of an averaged
    /// whole-note vector. The `dedupe_key` is the recording's `meeting_id` when
    /// it belongs to a meeting, else its own id — exposed so the fusion in
    /// [`Self::hybrid_search`] can collapse a meeting on the SAME key the lexical
    /// retriever uses, even if the two retrievers each pick a different track of
    /// that meeting as its representative (otherwise the meeting would surface
    /// twice). Recordings that only have a legacy whole-recording vector (no
    /// chunks yet, pending backfill) are folded in from the `embeddings` table so
    /// nothing becomes unsearchable during migration. Dimension-mismatched
    /// vectors are skipped (same guard as [`Self::semantic_search`]).
    async fn vector_ranking(&self, query_vec: &[f32]) -> Result<Vec<(String, RecordingId, f32)>> {
        let dim = query_vec.len();
        let query: Vec<f32> = query_vec.to_vec();
        // The decoded corpus (cached across queries; rebuilt after any write).
        let corpus = self.embedding_corpus().await?;

        // Best-chunk cosine over the whole corpus is CPU-bound — up to
        // MAX_CACHED_VECTORS dot products — so run it on the blocking pool rather
        // than inline on the async executor, where a large library would stall
        // IPC reads / audio streaming between await points.
        let best = tokio::task::spawn_blocking(move || {
            // best raw cosine per dedupe key (meeting_id or recording id).
            let mut best: std::collections::HashMap<String, (RecordingId, f32)> =
                std::collections::HashMap::new();

            // Score one pre-decoded vector into `best`. A corrupt blob (vector
            // None) or a dimension mismatch is skipped, exactly as the inline
            // decode did.
            let mut consider = |cv: &CachedVector| {
                let Some(vec) = cv.vector.as_deref() else {
                    return; // corrupt blob, already warned at load
                };
                if vec.len() != dim {
                    tracing::warn!(id = %cv.id, dim = vec.len(), query_dim = dim, "skipping embedding: dimension mismatch");
                    return;
                }
                let score = crate::embed::Embedder::cosine_similarity(&query, vec);
                let Some(rec_id) = RecordingId::parse(cv.id.clone()) else {
                    return;
                };
                let key = cv.meeting_id.clone().unwrap_or_else(|| cv.id.clone());
                best.entry(key)
                    .and_modify(|e| {
                        if score > e.1 {
                            *e = (rec_id.clone(), score);
                        }
                    })
                    .or_insert((rec_id, score));
            };

            // Per-chunk vectors (the primary, high-recall path).
            let mut have_chunks: std::collections::HashSet<&str> =
                std::collections::HashSet::new();
            for cv in &corpus.chunks {
                have_chunks.insert(cv.id.as_str());
                consider(cv);
            }

            // Legacy whole-recording vectors, only for recordings not yet chunked,
            // so the library stays searchable while the backfill runs.
            for cv in &corpus.legacy {
                if have_chunks.contains(cv.id.as_str()) {
                    continue; // chunks supersede the legacy whole-recording vector
                }
                consider(cv);
            }
            best
        })
        .await
        .map_err(|e| crate::error::Error::Internal(format!("vector ranking task failed: {e}")))?;

        let mut ranking: Vec<(String, RecordingId, f32)> = best
            .into_iter()
            .map(|(key, (rec_id, score))| (key, rec_id, score))
            .collect();
        ranking.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        Ok(ranking)
    }

    /// The lexical (FTS5) ranking for a query, meeting-deduped, best-first.
    ///
    /// FTS5 `rank` is BM25-like (more negative = more relevant), so ordering by
    /// `rank` ascending gives best-first. We keep the first (best) occurrence per
    /// dedupe key and return `(dedupe_key, RecordingId)` so the fusion in
    /// [`Self::hybrid_search`] collapses a meeting on the same key the vector
    /// retriever uses. This list feeds the RRF fusion as the "exact term"
    /// retriever that complements the paraphrase-oriented vector retriever.
    async fn lexical_ranking(&self, query: &str) -> Result<Vec<(String, RecordingId)>> {
        let sanitized = sanitize_fts5_query(query);
        if sanitized.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT r.id AS id, r.meeting_id AS meeting_id \
             FROM recordings_fts f \
             JOIN recordings r ON r.rowid = f.rowid \
             WHERE recordings_fts MATCH ? \
             ORDER BY f.rank",
        )
        .bind(&sanitized)
        .fetch_all(&self.pool)
        .await?;

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out = Vec::new();
        for row in rows {
            let id: String = row.try_get("id")?;
            let meeting_id: Option<String> = row.try_get("meeting_id")?;
            let key = meeting_id.unwrap_or_else(|| id.clone());
            if !seen.insert(key.clone()) {
                continue; // already have the best-ranked track of this meeting
            }
            if let Some(rec_id) = RecordingId::parse(id) {
                out.push((key, rec_id));
            }
        }
        Ok(out)
    }

    /// Recordings whose TAG name matches `query` (case-insensitive substring),
    /// meeting-deduped, in the same `(dedupe_key, RecordingId)` shape as
    /// [`Self::lexical_ranking`]. Feeds the hybrid search's lexical (exact-intent)
    /// side so a tag-name query surfaces its tagged recordings even in semantic
    /// mode — mirroring the tag clause the plain [`Self::list`] already applies.
    async fn tag_ranking(&self, query: &str) -> Result<Vec<(String, RecordingId)>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let like = format!("%{q}%");
        let rows = sqlx::query(
            "SELECT r.id AS id, r.meeting_id AS meeting_id \
             FROM recordings r \
             JOIN recording_tags rt ON rt.recording_id = r.id \
             JOIN tags t ON t.id = rt.tag_id \
             WHERE t.name LIKE ? \
             ORDER BY r.started_at DESC, r.id DESC",
        )
        .bind(&like)
        .fetch_all(&self.pool)
        .await?;

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out = Vec::new();
        for row in rows {
            let id: String = row.try_get("id")?;
            let meeting_id: Option<String> = row.try_get("meeting_id")?;
            let key = meeting_id.unwrap_or_else(|| id.clone());
            if !seen.insert(key.clone()) {
                continue;
            }
            if let Some(rec_id) = RecordingId::parse(id) {
                out.push((key, rec_id));
            }
        }
        Ok(out)
    }

    /// Hybrid semantic + lexical search with Reciprocal Rank Fusion.
    ///
    /// This is the search the daemon now uses. It:
    /// 1. Ranks recordings by best-matching chunk cosine (paraphrase recall).
    /// 2. Ranks recordings by FTS5 BM25 (exact-term recall).
    /// 3. Fuses the two rankings with RRF (no fragile cross-scale threshold).
    /// 4. Returns `(RecordingId, relevance)` where `relevance` is the *calibrated*
    ///    best-chunk cosine (0..1) for display — a meaningful percentage, not raw
    ///    cosine. Lexical-only hits (no vector signal) get a small floor relevance
    ///    so they still surface with an honest "weak semantic match" reading.
    ///
    /// Performs a hybrid search over the recording catalog, combining semantic
    /// (vector) search and lexical (FTS5) search results.
    ///
    /// This function implements Reciprocal Rank Fusion (RRF) to merge the ordered
    /// listings from two distinct retrievers:
    /// 1. A vector-based semantic retriever that scores using cosine similarity
    ///    over ONNX embedding chunks (see [`crate::embed::Embedder`]).
    /// 2. A lexical prefix query over the FTS5 full-text search virtual table.
    ///
    /// ### Meeting Collapsing
    /// In Meeting Mode, a single meeting has two separate tracks (microphone and
    /// system loopback). Returning both tracks as separate search results would
    /// clutter the UI. To prevent this, results are grouped by a stable deduplication
    /// key (the `meeting_id` for meetings, or the `id` for standalone voice notes).
    /// If both retrievers match different tracks of the same meeting, the results
    /// collapse, and we return a single representative `RecordingId` (preferring
    /// the track with the strongest semantic match).
    ///
    /// ### Relevance Calibration & Flooring
    /// The `min_relevance` parameter filters out weak semantic hits (whose calibrated
    /// cosine score falls below the floor). Crucially, exact term matches from the
    /// lexical retriever are exempt from this threshold — if a user searches for an
    /// exact word that is present in the transcript, it is returned even if the
    /// semantic similarity score is low.
    /// ### Optional filter (S3)
    /// When `filter` is `Some`, the fused results are restricted to recordings
    /// matching the same constraints as the plain [`Self::list`] — tag, status,
    /// date range, kind, favorite, in-place, tag-presence — so a meaning-search
    /// can be scoped exactly like the Library. The restriction is applied
    /// **after ranking, before the `limit` truncation**, so the top-`limit`
    /// in-scope results come back (not the top-`limit` overall, then thinned).
    /// A meeting passes when *either* track matches (the candidate set is keyed
    /// by the same meeting-stable dedupe key). The filter's query/pagination
    /// fields — `search` (the query is the separate `query`/`query_vec`),
    /// `limit`, `offset`, `sort_desc` — are ignored for the restriction; only
    /// its predicate fields scope the candidate set. `None` = today's behavior,
    /// unchanged.
    pub async fn hybrid_search(
        &self,
        query: &str,
        query_vec: &[f32],
        limit: usize,
        min_relevance: f32,
        filter: Option<&ListFilter>,
    ) -> Result<Vec<(RecordingId, f32)>> {
        // S3: when a filter is given, pre-compute the in-scope dedupe keys so the
        // fused ranking can be restricted to them. Built from the SAME `list`
        // query the Library uses (predicate fields only — query/pagination are
        // dropped), then mapped to dedupe keys so a meeting passes if either of
        // its tracks matches.
        let allowed_keys: Option<std::collections::HashSet<String>> = match filter {
            Some(f) => {
                let scoped = ListFilter {
                    // Drop query + pagination + sort: this list is only used to
                    // derive the in-scope candidate set, not to order/page it.
                    search: None,
                    limit: None,
                    offset: None,
                    sort_desc: None,
                    ..f.clone()
                };
                let rows = self.list(&scoped).await?;
                Some(
                    rows.into_iter()
                        .map(|r| r.meeting_id.unwrap_or_else(|| r.id.as_str().to_string()))
                        .collect(),
                )
            }
            None => None,
        };

        let vec_rank = self.vector_ranking(query_vec).await?;
        let mut lex_rank = self.lexical_ranking(query).await?;
        // Fold tag-name matches into the lexical (exact-intent) set so searching
        // a tag surfaces its recordings in semantic mode too — the plain `list()`
        // already does this for non-semantic search. Deduped by key and appended
        // after the FTS hits, so true transcript matches keep their stronger rank.
        {
            let mut seen: std::collections::HashSet<String> =
                lex_rank.iter().map(|(k, _)| k.clone()).collect();
            for (key, id) in self.tag_ranking(query).await? {
                if seen.insert(key.clone()) {
                    lex_rank.push((key, id));
                }
            }
        }

        // Everything below is keyed by the meeting-stable DEDUPE KEY (meeting_id
        // or recording id), not the raw recording id, so a meeting collapses to a
        // single result even when the vector and lexical retrievers each pick a
        // different track of it as their representative.

        // dedupe_key -> best raw cosine (for calibration into a relevance %).
        let cosine_by_key: std::collections::HashMap<String, f32> = vec_rank
            .iter()
            .map(|(key, _id, c)| (key.clone(), *c))
            .collect();
        // dedupe_key -> a representative RecordingId to return for that key.
        // Prefer the vector retriever's pick (best-chunk track); fall back to the
        // lexical retriever's for lexical-only hits.
        let mut rec_id_by_key: std::collections::HashMap<String, RecordingId> =
            std::collections::HashMap::new();
        for (key, id, _c) in &vec_rank {
            rec_id_by_key
                .entry(key.clone())
                .or_insert_with(|| id.clone());
        }
        for (key, id) in &lex_rank {
            rec_id_by_key
                .entry(key.clone())
                .or_insert_with(|| id.clone());
        }
        let lexical_keys: std::collections::HashSet<String> =
            lex_rank.iter().map(|(key, _id)| key.clone()).collect();

        // Fuse the two orderings on the dedupe key.
        let vec_keys: Vec<String> = vec_rank.iter().map(|(key, _, _)| key.clone()).collect();
        let lex_keys: Vec<String> = lex_rank.iter().map(|(key, _)| key.clone()).collect();
        // Weight the semantic list slightly higher: the whole point is paraphrase
        // recall, and the lexical list is the complementary safety net.
        let fused = crate::fusion::reciprocal_rank_fusion(
            &[&vec_keys[..], &lex_keys[..]],
            Some(&[1.0, 0.85]),
        );

        // Small relevance floor for a lexical-only hit so it surfaces honestly
        // rather than reading "0% relevant" despite being an exact-term match.
        const LEXICAL_ONLY_RELEVANCE: f32 = 0.30;

        let mut results: Vec<(RecordingId, f32)> = Vec::new();
        for (key, _fused_score) in fused {
            // S3: restrict to the in-scope candidate set when a filter was given.
            // Applied here — after ranking, before the `truncate(limit)` below —
            // so the top in-scope results survive rather than the top overall.
            if let Some(allowed) = &allowed_keys {
                if !allowed.contains(&key) {
                    continue;
                }
            }
            let Some(rec_id) = rec_id_by_key.get(&key).cloned() else {
                continue;
            };
            let is_lexical = lexical_keys.contains(&key);
            let relevance = match cosine_by_key.get(&key) {
                Some(c) => crate::fusion::calibrate_cosine(*c),
                None => 0.0,
            };
            // A lexical hit is kept regardless of its (possibly weak) cosine; a
            // semantic-only hit must clear the relevance floor.
            let display = if is_lexical {
                relevance.max(LEXICAL_ONLY_RELEVANCE)
            } else {
                relevance
            };
            if !is_lexical && display < min_relevance {
                continue;
            }
            results.push((rec_id, display));
        }
        results.truncate(limit);
        Ok(results)
    }

    /// "More like this": rank the library by semantic similarity to a stored
    /// recording, reusing its already-stored vectors — no fresh embedding, so
    /// it costs one corpus scan and works even when the embedding model isn't
    /// loaded.
    ///
    /// The query vector is the **mean of the source's chunk vectors**,
    /// L2-renormalized. The centroid captures what the whole note is about,
    /// while candidates are still scored by their own best-matching chunk via
    /// `vector_ranking` — the exact retrieval path a typed semantic
    /// query takes — so a long candidate ranks on its closest idea instead of
    /// an averaged blur. A source that only has a legacy whole-recording
    /// vector uses that vector directly (it *is* that recording's mean).
    ///
    /// The source never appears in the results. Exclusion is by the
    /// meeting-stable dedupe key, so for a meeting track the *other* track of
    /// the same meeting — a near-identical transcript that would trivially
    /// rank #1 — is excluded too. Scores are calibrated like a normal
    /// semantic search ([`crate::fusion::calibrate_cosine`]); hits under
    /// `min_relevance` are dropped, and at most `limit` results return,
    /// best-first.
    ///
    /// Errors: [`crate::error::Error::NotFound`] when `id` doesn't exist, and
    /// a "not indexed yet" [`crate::error::Error::Internal`] when the
    /// recording has no usable stored vectors (not embedded yet, or every
    /// blob was corrupt) — the caller can surface that message as-is.
    pub async fn more_like_this(
        &self,
        id: &RecordingId,
        limit: usize,
        min_relevance: f32,
    ) -> Result<Vec<(RecordingId, f32)>> {
        // Resolve the source row first so a missing id reports NotFound rather
        // than "not indexed", and grab its meeting for the dedupe-key exclusion.
        let row = sqlx::query("SELECT meeting_id FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Err(crate::error::Error::NotFound { id: id.to_string() });
        };
        let meeting_id: Option<String> = row.try_get("meeting_id")?;
        let source_key = meeting_id.unwrap_or_else(|| id.as_str().to_string());

        // The source's stored chunk vectors; a not-yet-chunked recording falls
        // back to its legacy whole-recording vector (same precedence as
        // `vector_ranking`).
        let mut rows = sqlx::query(
            "SELECT vector FROM embedding_chunks WHERE recording_id = ? ORDER BY chunk_index",
        )
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            rows = sqlx::query("SELECT vector FROM embeddings WHERE id = ?")
                .bind(id.as_str())
                .fetch_all(&self.pool)
                .await?;
        }

        // Component-wise mean of the source vectors, skipping any corrupt or
        // odd-dimension blob (same guards as the search paths), then
        // L2-renormalize — cosine is a plain dot product over unit vectors, and
        // a mean of unit vectors is shorter than unit.
        let mut mean: Vec<f32> = Vec::new();
        let mut count = 0usize;
        for row in rows {
            let bytes: Vec<u8> = row.try_get("vector")?;
            if !bytes.len().is_multiple_of(4) {
                tracing::warn!(id = %id, len = bytes.len(), "skipping source embedding: not 4-byte aligned");
                continue;
            }
            let vec: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            if mean.is_empty() {
                mean = vec;
                count = 1;
            } else if vec.len() == mean.len() {
                for (m, v) in mean.iter_mut().zip(&vec) {
                    *m += v;
                }
                count += 1;
            } else {
                tracing::warn!(id = %id, dim = vec.len(), mean_dim = mean.len(), "skipping source embedding: dimension mismatch");
            }
        }
        if count == 0 {
            return Err(crate::error::Error::Internal(format!(
                "recording {id} isn't indexed for semantic search yet — re-embed the library or wait for the pipeline to index it"
            )));
        }
        for m in &mut mean {
            *m /= count as f32;
        }
        crate::embed::l2_normalize(&mut mean);

        // Score every OTHER recording by its best chunk against the centroid.
        let ranking = self.vector_ranking(&mean).await?;
        let mut results: Vec<(RecordingId, f32)> = Vec::new();
        for (key, rec_id, cosine) in ranking {
            if results.len() >= limit {
                break;
            }
            if key == source_key {
                continue; // never recommend the source (or its meeting sibling)
            }
            let relevance = crate::fusion::calibrate_cosine(cosine);
            if relevance < min_relevance {
                continue;
            }
            results.push((rec_id, relevance));
        }
        Ok(results)
    }

    /// List recordings matching `filter`, newest-first by default, with tags and
    /// speaker names populated per row.
    ///
    /// Every predicate — full-text search, tag, status, kind (single vs.
    /// meeting), favorites, date range — is applied in SQL *before* `LIMIT`/
    /// `OFFSET`, so pagination composes correctly (filtering after pagination
    /// would return mostly-empty pages of the chosen kind). Backs the GUI
    /// Library and the CLI `phoneme list`.
    pub async fn list(&self, filter: &ListFilter) -> Result<Vec<Recording>> {
        let mut sql = String::from("SELECT recordings.* FROM recordings");

        let mut fts_query = None;
        let mut tag_search_query = None;
        let mut model_search_query = None;

        if let Some(q) = filter.search.as_deref() {
            let sanitized = sanitize_fts5_query(q);
            if !sanitized.is_empty() {
                fts_query = Some(sanitized);
                let like = format!("%{}%", q);
                tag_search_query = Some(like.clone());
                // The same substring also matches any step's model name, so a
                // search like "large-v3" or "gemma3:4b" finds everything that
                // model ran on (see the WHERE OR + per-column binds below).
                model_search_query = Some(like);
            }
        }

        if filter.tag_id.is_some() {
            sql.push_str(" JOIN recording_tags rt ON rt.recording_id = recordings.id");
        }

        sql.push_str(" WHERE 1=1");

        if fts_query.is_some() {
            sql.push_str(" AND (recordings.rowid IN (SELECT rowid FROM recordings_fts WHERE transcript MATCH ?) OR recordings.id IN (SELECT recording_id FROM recording_tags rts JOIN tags ts ON ts.id = rts.tag_id WHERE ts.name LIKE ?) OR recordings.model LIKE ? OR recordings.cleanup_model LIKE ? OR recordings.summary_model LIKE ? OR recordings.title_model LIKE ? OR recordings.tag_model LIKE ? OR recordings.diarization_model LIKE ?)");
        }
        if let Some(tag_id) = filter.tag_id {
            // `tag_id` is `i64`, so direct formatting is injection-safe (an
            // integer can't carry SQL) — same rationale as the `u32` LIMIT/OFFSET
            // below. String filters (FTS, status) use bound `?` parameters.
            sql.push_str(&format!(" AND rt.tag_id = {tag_id}"));
        }
        if filter.status.is_some() {
            sql.push_str(" AND recordings.status = ?");
        }
        // Kind + favorites are filtered HERE, not client-side: they must apply
        // before LIMIT/OFFSET or pages of the chosen kind come back mostly
        // empty (post-pagination filtering only ever thins the fetched page).
        match filter.kind {
            Some(crate::types::ListKind::Single) => {
                sql.push_str(" AND recordings.meeting_id IS NULL");
            }
            Some(crate::types::ListKind::Meeting) => {
                sql.push_str(" AND recordings.meeting_id IS NOT NULL");
            }
            None => {}
        }
        match filter.favorite {
            Some(true) => sql.push_str(" AND recordings.favorite = 1"),
            Some(false) => sql.push_str(" AND recordings.favorite = 0"),
            None => {}
        }
        match filter.in_place {
            Some(true) => sql.push_str(" AND recordings.in_place = 1"),
            Some(false) => sql.push_str(" AND recordings.in_place = 0"),
            None => {}
        }
        // Tag-presence filter (sidebar "All Tags" / "Untagged"). A static subquery
        // over recording_tags — no bound params, injection-safe. Independent of
        // the `tag_id` (single-tag) JOIN above, so the two compose.
        match filter.tagged {
            Some(true) => {
                sql.push_str(" AND recordings.id IN (SELECT recording_id FROM recording_tags)")
            }
            Some(false) => {
                sql.push_str(" AND recordings.id NOT IN (SELECT recording_id FROM recording_tags)")
            }
            None => {}
        }
        if filter.since.is_some() {
            sql.push_str(" AND recordings.started_at >= ?");
        }
        if filter.until.is_some() {
            sql.push_str(" AND recordings.started_at <= ?");
        }
        let dir = if filter.sort_desc.unwrap_or(true) {
            "DESC"
        } else {
            "ASC"
        };
        sql.push_str(&format!(
            " ORDER BY recordings.started_at {dir}, recordings.id {dir}"
        ));
        // LIMIT / OFFSET for pagination. SQLite requires a LIMIT before an
        // OFFSET, so when only an offset is given we use `LIMIT -1` (= no row
        // cap). `limit`/`offset` are `u32`, so direct formatting is injection-safe.
        match (filter.limit, filter.offset) {
            (Some(n), Some(m)) => sql.push_str(&format!(" LIMIT {n} OFFSET {m}")),
            (Some(n), None) => sql.push_str(&format!(" LIMIT {n}")),
            (None, Some(m)) => sql.push_str(&format!(" LIMIT -1 OFFSET {m}")),
            (None, None) => {}
        }

        let mut q = sqlx::query(&sql);
        if let Some(fq) = &fts_query {
            q = q.bind(fq);
        }
        if let Some(tq) = &tag_search_query {
            q = q.bind(tq);
        }
        if let Some(mq) = &model_search_query {
            // One bind per model column in the WHERE OR above (transcription +
            // cleanup + summary + title + tag + diarization), in that order.
            for _ in 0..6 {
                q = q.bind(mq);
            }
        }
        if let Some(s) = filter.status {
            q = q.bind(s.as_str().to_string());
        }
        if let Some(t) = filter.since {
            q = q.bind(t.to_rfc3339());
        }
        if let Some(t) = filter.until {
            q = q.bind(t.to_rfc3339());
        }
        let rows = q.fetch_all(&self.pool).await?;
        let mut recs: Vec<Recording> = rows
            .into_iter()
            .map(row_to_recording)
            .collect::<Result<_>>()?;
        // Populate tags + custom speaker names for each recording (N+1 query;
        // acceptable for desktop UI scale).
        for rec in &mut recs {
            rec.tags = self.tags_for(&rec.id).await.unwrap_or_default();
            rec.speaker_names = self.speaker_names_for(&rec.id).await.unwrap_or_default();
        }
        Ok(recs)
    }

    /// Fetch all recordings belonging to a single meeting session.
    ///
    /// Returns the rows that share `meeting_id`, ordered by `track` then
    /// `started_at` so the two tracks of a meeting come back in a stable order
    /// (e.g. "mic" before "system", since "mic" < "system" lexicographically).
    /// A `meeting_id` with no rows yields an empty `Vec` (not an error) — the
    /// caller treats that as "no such session".
    pub async fn list_by_meeting(&self, meeting_id: &str) -> Result<Vec<Recording>> {
        let rows = sqlx::query(
            "SELECT * FROM recordings WHERE meeting_id = ? \
             ORDER BY track ASC, started_at ASC, id ASC",
        )
        .bind(meeting_id)
        .fetch_all(&self.pool)
        .await?;
        let mut recs: Vec<Recording> = rows
            .into_iter()
            .map(row_to_recording)
            .collect::<Result<_>>()?;
        // The merged meeting view maps `[Speaker N]` → custom names per track, so
        // each track must carry its own speaker-name map.
        for rec in &mut recs {
            rec.speaker_names = self.speaker_names_for(&rec.id).await.unwrap_or_default();
        }
        Ok(recs)
    }

    /// Delete a recording's catalog row. Cascading foreign keys take its tags,
    /// segments, speaker names, and embeddings with it; the caller removes the
    /// audio file from disk separately.
    pub async fn delete(&self, id: &RecordingId) -> Result<()> {
        // Named voices that will lose a sample when the cascade removes this
        // recording's voiceprints — captured BEFORE the delete so we can recompute
        // their cached centroids afterward (audit H1). NULL links are skipped.
        let affected: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT named_voice_id FROM speaker_voiceprints \
             WHERE recording_id = ? AND named_voice_id IS NOT NULL",
        )
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await?;

        sqlx::query("DELETE FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .execute(&self.pool)
            .await?;
        // The cascade took this recording's embeddings, voiceprints, and dismissed
        // suggestions with it — drop the search snapshot, then recompute any named
        // voice that just lost a sample so its centroid + count stay accurate.
        self.invalidate_embedding_cache();
        for nid in affected {
            self.recompute_named_centroid(&nid).await?;
        }
        Ok(())
    }

    /// Delete EVERY recording row — and, via the same cascade as [`Self::delete`], all
    /// their tags, segments, words, speaker names, and embeddings. Used by the
    /// destructive catalog rebuild, which then re-imports the audio from disk.
    /// Returns the number of rows removed. The caller leaves the WAV files on
    /// disk (the rebuild re-links them).
    pub async fn clear_all_recordings(&self) -> Result<u64> {
        // Named voices that will lose samples when the cascade removes every
        // recording's voiceprints — captured BEFORE the delete so their cached
        // centroids + counts can be recomputed afterward, mirroring
        // [`Self::delete`] (audit M1). NULL links are skipped.
        let affected: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT named_voice_id FROM speaker_voiceprints \
             WHERE named_voice_id IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?;

        let res = sqlx::query("DELETE FROM recordings")
            .execute(&self.pool)
            .await?;
        self.invalidate_embedding_cache();
        for nid in affected {
            self.recompute_named_centroid(&nid).await?;
        }
        Ok(res.rows_affected())
    }

    /// Run an explicit WAL checkpoint. PASSIVE mode is non-blocking — readers
    /// can keep going while the checkpoint runs. The daemon calls this on idle
    /// (e.g., when the queue worker has been quiet for a few minutes) to keep
    /// the `-wal` file from growing unbounded under sustained read pressure
    /// from `SubscribeEvents` subscribers.
    pub async fn checkpoint(&self) -> Result<()> {
        sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Tags attached to at least one recording, name-sorted. Powers the filter
    /// dropdown and tag autocomplete — orphaned tags are excluded (see
    /// [`Catalog::list_all_tags`] for the unfiltered set).
    pub async fn list_tags(&self) -> Result<Vec<Tag>> {
        // Only return tags that are attached to at least one recording.
        // Orphaned tags (detached from all recordings) are excluded so they
        // don't pollute the filter dropdown or tag autocomplete.
        let rows = sqlx::query(
            "SELECT id, name, color FROM tags \
             WHERE id IN (SELECT tag_id FROM recording_tags) \
             ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(Tag {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    color: r.try_get("color")?,
                })
            })
            .collect()
    }

    /// Create a tag named `name`, or return the existing one (matching is
    /// case-insensitive, so adding "code" when "Code" exists reuses it, colour
    /// and links intact).
    pub async fn add_tag(&self, name: &str, color: Option<&str>) -> Result<Tag> {
        // Tags are case-INSENSITIVELY unique at the application level: "Code"
        // and "code" are the same tag, so adding either reuses the existing
        // row (keeping its color and recording links — the first-created
        // casing wins). The UNIQUE index on `name` is byte-wise, which is why
        // this lookup guards the insert. COLLATE NOCASE is ASCII-only; that
        // covers the realistic duplicate ("Test"/"test") without rewriting
        // non-ASCII tag names.
        let existing =
            sqlx::query("SELECT id, name, color FROM tags WHERE name = ? COLLATE NOCASE")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        if let Some(row) = existing {
            return Ok(Tag {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                color: row.try_get("color")?,
            });
        }
        sqlx::query("INSERT OR IGNORE INTO tags (name, color) VALUES (?, ?)")
            .bind(name)
            .bind(color)
            .execute(&self.pool)
            .await?;
        let row = sqlx::query("SELECT id, name, color FROM tags WHERE name = ?")
            .bind(name)
            .fetch_one(&self.pool)
            .await?;
        Ok(Tag {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            color: row.try_get("color")?,
        })
    }

    /// Rename and/or recolour an existing tag, returning the updated record.
    pub async fn update_tag(&self, id: i64, name: &str, color: Option<&str>) -> Result<Tag> {
        sqlx::query("UPDATE tags SET name = ?, color = ? WHERE id = ?")
            .bind(name)
            .bind(color)
            .bind(id)
            .execute(&self.pool)
            .await?;
        let row = sqlx::query("SELECT id, name, color FROM tags WHERE id = ?")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        Ok(Tag {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            color: row.try_get("color")?,
        })
    }

    /// Delete a tag entirely; cascading foreign keys detach it from every
    /// recording it was on.
    pub async fn delete_tag(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM tags WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Every recording id currently in the catalog — for de-duping a disk
    /// re-import (`ReimportFromDisk`) against rows that already exist. Ids that
    /// somehow fail the canonical shape check are skipped rather than panicking.
    pub async fn all_ids(&self) -> Result<Vec<RecordingId>> {
        let rows = sqlx::query("SELECT id FROM recordings")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let s: String = r.try_get("id").ok()?;
                RecordingId::parse(s)
            })
            .collect())
    }

    /// Returns ALL tags including ones not attached to any recording.
    /// Used by the Tag Manager settings UI.
    pub async fn list_all_tags(&self) -> Result<Vec<Tag>> {
        let rows = sqlx::query("SELECT id, name, color FROM tags ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|r| {
                Ok(Tag {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    color: r.try_get("color")?,
                })
            })
            .collect()
    }

    /// Attach a tag to a recording (idempotent — attaching an already-attached
    /// tag is a no-op).
    pub async fn attach_tag(&self, recording_id: &RecordingId, tag_id: i64) -> Result<()> {
        sqlx::query("INSERT OR IGNORE INTO recording_tags (recording_id, tag_id) VALUES (?, ?)")
            .bind(recording_id.as_str())
            .bind(tag_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Detach a tag from a recording (the tag itself is left intact).
    pub async fn detach_tag(&self, recording_id: &RecordingId, tag_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM recording_tags WHERE recording_id = ? AND tag_id = ?")
            .bind(recording_id.as_str())
            .bind(tag_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Number of recordings each tag is attached to, keyed by tag id. Tags with
    /// no attachments are simply absent from the map (treated as zero by callers).
    /// Powers the Tag Manager usage counts.
    pub async fn tag_usage_counts(&self) -> Result<std::collections::HashMap<i64, i64>> {
        let rows =
            sqlx::query("SELECT tag_id, COUNT(*) AS cnt FROM recording_tags GROUP BY tag_id")
                .fetch_all(&self.pool)
                .await?;
        let mut map = std::collections::HashMap::with_capacity(rows.len());
        for r in rows {
            let id: i64 = r.try_get("tag_id")?;
            let cnt: i64 = r.try_get("cnt")?;
            map.insert(id, cnt);
        }
        Ok(map)
    }

    /// Full-corpus recording counts per Library kind (all / single / meeting /
    /// in-place / favorite), computed in one SQL pass. Powers the sidebar's
    /// Library count badges (the GUI counterpart of `tag_usage_counts`).
    pub async fn kind_counts(&self) -> Result<crate::types::KindCounts> {
        let row = sqlx::query(
            "SELECT
                COUNT(*) AS all_count,
                COALESCE(SUM(CASE WHEN meeting_id IS NULL THEN 1 ELSE 0 END), 0) AS single_count,
                COALESCE(SUM(CASE WHEN meeting_id IS NOT NULL THEN 1 ELSE 0 END), 0) AS meeting_count,
                COALESCE(SUM(CASE WHEN in_place = 1 THEN 1 ELSE 0 END), 0) AS in_place_count,
                COALESCE(SUM(CASE WHEN favorite = 1 THEN 1 ELSE 0 END), 0) AS favorite_count,
                (SELECT COUNT(DISTINCT recording_id) FROM recording_tags) AS tagged_count,
                (COUNT(*) - (SELECT COUNT(DISTINCT recording_id) FROM recording_tags)) AS untagged_count
             FROM recordings",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(crate::types::KindCounts {
            all: row.try_get("all_count")?,
            single: row.try_get("single_count")?,
            meeting: row.try_get("meeting_count")?,
            in_place: row.try_get("in_place_count")?,
            favorite: row.try_get("favorite_count")?,
            tagged: row.try_get("tagged_count")?,
            untagged: row.try_get("untagged_count")?,
        })
    }

    /// Merge `from_id` into `into_id`: every recording tagged `from_id` becomes
    /// tagged `into_id` (de-duplicated), then `from_id` is deleted. A no-op when
    /// the two ids are equal. Used by the Tag Manager's merge action.
    pub async fn merge_tags(&self, from_id: i64, into_id: i64) -> Result<()> {
        if from_id == into_id {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        // Re-point every link from the source tag onto the target, skipping rows
        // that would collide with an existing (recording_id, into_id) pair.
        sqlx::query(
            "INSERT OR IGNORE INTO recording_tags (recording_id, tag_id) \
             SELECT recording_id, ? FROM recording_tags WHERE tag_id = ?",
        )
        .bind(into_id)
        .bind(from_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM recording_tags WHERE tag_id = ?")
            .bind(from_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM tags WHERE id = ?")
            .bind(from_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Apply the configured retention policy, removing eligible recordings from
    /// the catalog and returning their `audio_path` values so the caller can
    /// delete the files from disk.
    ///
    /// Only terminal-state recordings (done / failed / cancelled) are eligible —
    /// in-progress recordings are always preserved regardless of age or count.
    pub async fn apply_retention(
        &self,
        cfg: &crate::config::RetentionConfig,
    ) -> Result<Vec<String>> {
        let mut deleted_paths: Vec<String> = Vec::new();
        // Tracks whether any row was hard-deleted (not audio-only). A hard
        // delete cascade-drops that recording's embeddings, so the warm cache
        // must be dropped or deleted vectors keep surfacing in search.
        let mut hard_deleted = false;
        // Named voices that lose a sample to a hard delete — collected as we go
        // (BEFORE each delete cascades the voiceprint away) so their centroids +
        // counts can be recomputed once at the end, mirroring [`Self::delete`]
        // (audit M1). A `HashSet` dedupes voices touched by several recordings.
        let mut affected_voices: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        // `delete_audio = true` is the disk-saver mode: the catalog row stays
        // (transcript searchable forever), only the WAV goes. The row's
        // audio_path is blanked so the UI doesn't offer a dead player, and so
        // the row never matches a later sweep again. `false` (default) deletes
        // row + audio together. This flag was previously ignored — audio-only
        // users were losing their rows.
        let audio_only = cfg.delete_audio;

        // Age-based cleanup — everything older than max_age_days.
        if let Some(max_age) = cfg.max_age_days {
            let cutoff =
                chrono::Utc::now() - chrono::Duration::try_days(max_age as i64).unwrap_or_default();
            let cutoff_str = cutoff.to_rfc3339();
            let rows = sqlx::query(&format!(
                "SELECT id, audio_path FROM recordings \
                 WHERE started_at < ? \
                 AND status IN ({}) \
                 AND audio_path != ''",
                RecordingStatus::terminal_sql_list()
            ))
            .bind(&cutoff_str)
            .fetch_all(&self.pool)
            .await?;
            for row in rows {
                let id: String = row.try_get("id")?;
                let audio_path: String = row.try_get("audio_path")?;
                if audio_only {
                    sqlx::query(
                        "UPDATE recordings SET audio_path = '', updated_at = datetime('now') \
                         WHERE id = ?",
                    )
                    .bind(&id)
                    .execute(&self.pool)
                    .await?;
                } else {
                    // Capture the named voices losing a sample BEFORE the cascade
                    // removes this recording's voiceprints (audit M1).
                    let voices: Vec<String> = sqlx::query_scalar(
                        "SELECT DISTINCT named_voice_id FROM speaker_voiceprints \
                         WHERE recording_id = ? AND named_voice_id IS NOT NULL",
                    )
                    .bind(&id)
                    .fetch_all(&self.pool)
                    .await?;
                    affected_voices.extend(voices);
                    sqlx::query("DELETE FROM recordings WHERE id = ?")
                        .bind(&id)
                        .execute(&self.pool)
                        .await?;
                    hard_deleted = true;
                }
                deleted_paths.push(audio_path);
            }
        }

        // Count-based cleanup — all but the most recent max_count. In
        // audio-only mode the ranking still counts EVERY terminal row (rows
        // are kept, so "the most recent N" must mean recordings, not files) —
        // the audio_path filter above/below only stops re-processing rows
        // whose audio is already gone.
        if let Some(max_count) = cfg.max_count {
            let rows = sqlx::query(&format!(
                "SELECT id, audio_path FROM recordings \
                 WHERE status IN ({}) \
                 ORDER BY started_at DESC, id DESC \
                 LIMIT -1 OFFSET ?",
                RecordingStatus::terminal_sql_list()
            ))
            .bind(max_count as i64)
            .fetch_all(&self.pool)
            .await?;
            for row in rows {
                let id: String = row.try_get("id")?;
                let audio_path: String = row.try_get("audio_path")?;
                if audio_path.is_empty() {
                    continue; // audio already reclaimed by an earlier sweep
                }
                if audio_only {
                    sqlx::query(
                        "UPDATE recordings SET audio_path = '', updated_at = datetime('now') \
                         WHERE id = ?",
                    )
                    .bind(&id)
                    .execute(&self.pool)
                    .await?;
                } else {
                    // Capture the named voices losing a sample BEFORE the cascade
                    // removes this recording's voiceprints (audit M1).
                    let voices: Vec<String> = sqlx::query_scalar(
                        "SELECT DISTINCT named_voice_id FROM speaker_voiceprints \
                         WHERE recording_id = ? AND named_voice_id IS NOT NULL",
                    )
                    .bind(&id)
                    .fetch_all(&self.pool)
                    .await?;
                    affected_voices.extend(voices);
                    sqlx::query("DELETE FROM recordings WHERE id = ?")
                        .bind(&id)
                        .execute(&self.pool)
                        .await?;
                    hard_deleted = true;
                }
                deleted_paths.push(audio_path);
            }
        }

        // A hard delete cascade-drops the recordings' embeddings; drop the warm
        // snapshot so deleted vectors stop surfacing in semantic search. (The
        // audio-only path keeps the rows + embeddings, so it needs no drop.)
        if hard_deleted {
            self.invalidate_embedding_cache();
        }
        // ...and recompute any named voice that just lost a sample so the Speaker
        // Library's cached centroids + counts stay accurate (audit M1).
        for nid in affected_voices {
            self.recompute_named_centroid(&nid).await?;
        }

        Ok(deleted_paths)
    }

    /// Predicts how many recordings will be deleted by the age-based retention policy
    /// in the next `hours_ahead` hours.
    pub async fn analyze_upcoming_retention(
        &self,
        cfg: &crate::config::RetentionConfig,
        hours_ahead: u32,
    ) -> Result<u32> {
        let max_age = match cfg.max_age_days {
            Some(v) => v,
            None => return Ok(0),
        };

        // cutoff_now is items older than this are ALREADY deleted or being deleted now.
        let cutoff_now =
            chrono::Utc::now() - chrono::Duration::try_days(max_age as i64).unwrap_or_default();
        // cutoff_future is items older than this will be deleted in the next `hours_ahead` hours.
        let cutoff_future =
            cutoff_now + chrono::Duration::try_hours(hours_ahead as i64).unwrap_or_default();

        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM recordings \
             WHERE started_at >= ? AND started_at < ? \
             AND status IN ({})",
            RecordingStatus::terminal_sql_list()
        ))
        .bind(cutoff_now.to_rfc3339())
        .bind(cutoff_future.to_rfc3339())
        .fetch_one(&self.pool)
        .await?;

        Ok(count as u32)
    }

    /// The tags attached to one recording, name-sorted. Used to fill
    /// `Recording::tags` and back the detail view.
    pub async fn tags_for(&self, recording_id: &RecordingId) -> Result<Vec<Tag>> {
        let rows = sqlx::query(
            r#"SELECT t.id, t.name, t.color
               FROM tags t
               JOIN recording_tags rt ON rt.tag_id = t.id
               WHERE rt.recording_id = ?
               ORDER BY t.name"#,
        )
        .bind(recording_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(Tag {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    color: r.try_get("color")?,
                })
            })
            .collect()
    }

    /// Set (or clear) the custom display name for one diarized speaker label.
    ///
    /// `speaker_label` is the 1-based index from the transcript's `[Speaker N]`
    /// marker. A non-empty `name` upserts the mapping; a blank/whitespace-only
    /// `name` deletes it (the label reverts to the default "Speaker N"). The
    /// stored transcript is never touched — names are applied at display/export
    /// time — so renaming is fully reversible. The recording is expected to
    /// exist; a foreign-key violation surfaces as an error.
    pub async fn set_speaker_name(
        &self,
        recording_id: &RecordingId,
        speaker_label: i64,
        name: &str,
    ) -> Result<()> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            sqlx::query("DELETE FROM speaker_names WHERE recording_id = ? AND speaker_label = ?")
                .bind(recording_id.as_str())
                .bind(speaker_label)
                .execute(&self.pool)
                .await?;
        } else {
            sqlx::query(
                "INSERT INTO speaker_names (recording_id, speaker_label, name) VALUES (?, ?, ?) \
                 ON CONFLICT(recording_id, speaker_label) DO UPDATE SET name = excluded.name",
            )
            .bind(recording_id.as_str())
            .bind(speaker_label)
            .bind(trimmed)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Seed a default display name for a speaker label only if none exists yet —
    /// `INSERT ... ON CONFLICT DO NOTHING`, so an existing row is left untouched.
    ///
    /// This is the pipeline's "friendly default" path (the meeting mic track's
    /// label 1 → "You"). Unlike [`Self::set_speaker_name`] (the user/IPC rename
    /// path, an upsert), this NEVER overwrites a name already on the row, so a
    /// user rename of that speaker survives a retranscribe / re-run that
    /// re-seeds the same default. The `name` is trimmed like `set_speaker_name`;
    /// a blank/whitespace-only `name` is a no-op (we never seed an empty
    /// default). The recording is expected to exist; a foreign-key violation
    /// surfaces as an error.
    pub async fn set_speaker_name_if_absent(
        &self,
        recording_id: &RecordingId,
        speaker_label: i64,
        name: &str,
    ) -> Result<()> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO speaker_names (recording_id, speaker_label, name) VALUES (?, ?, ?) \
             ON CONFLICT(recording_id, speaker_label) DO NOTHING",
        )
        .bind(recording_id.as_str())
        .bind(speaker_label)
        .bind(trimmed)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// All custom speaker names for a recording, ordered by speaker index. Empty
    /// when none have been set. Used to populate `Recording::speaker_names` and
    /// by the IPC layer so the frontend can map `[Speaker N]` → name at display
    /// and export time.
    pub async fn speaker_names_for(&self, recording_id: &RecordingId) -> Result<Vec<SpeakerName>> {
        let rows = sqlx::query(
            "SELECT speaker_label, name FROM speaker_names \
             WHERE recording_id = ? ORDER BY speaker_label",
        )
        .bind(recording_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(SpeakerName {
                    speaker_label: r.try_get("speaker_label")?,
                    name: r.try_get("name")?,
                })
            })
            .collect()
    }

    // ---- Speaker voiceprints — cross-recording named-speaker recognition (#9) ----

    /// Store (or refresh) the captured centroid for one speaker in a recording.
    /// The pipeline calls this for each labelled speaker after local diarization.
    /// An existing `named_voice_id` link is preserved (a re-transcribe refreshes
    /// the sample without un-enrolling), and the linked named voice is recomputed
    /// so its cached centroid tracks the new sample.
    ///
    /// `duration_ms` is the speaker's total speaking time in this recording — the
    /// duration-weight (roadmap V4) so a long, clean capture outvotes a brief one
    /// when the named voice is recomputed. Pass `0` when it isn't known; the
    /// weighted mean treats `0` as the equal-weight fallback (legacy behavior).
    pub async fn save_speaker_voiceprint(
        &self,
        recording_id: &str,
        speaker_label: i64,
        centroid: &[f32],
        duration_ms: i64,
    ) -> Result<()> {
        let json = serde_json::to_string(centroid)?;
        sqlx::query(
            "INSERT INTO speaker_voiceprints (recording_id, speaker_label, centroid, duration_ms) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(recording_id, speaker_label) DO UPDATE SET \
                 centroid = excluded.centroid, duration_ms = excluded.duration_ms",
        )
        .bind(recording_id)
        .bind(speaker_label)
        .bind(&json)
        .bind(duration_ms)
        .execute(&self.pool)
        .await?;
        if let Some(nid) = self.named_voice_for(recording_id, speaker_label).await? {
            self.recompute_named_centroid(&nid).await?;
        }
        Ok(())
    }

    /// The captured centroid for one speaker in a recording, if one exists.
    pub async fn speaker_voiceprint(
        &self,
        recording_id: &str,
        speaker_label: i64,
    ) -> Result<Option<Vec<f32>>> {
        let row = sqlx::query(
            "SELECT centroid FROM speaker_voiceprints \
             WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(recording_id)
        .bind(speaker_label)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(serde_json::from_str::<Vec<f32>>(
                &r.try_get::<String, _>("centroid")?,
            )?)),
            None => Ok(None),
        }
    }

    /// The named-voice id a recording's speaker is enrolled under, if any.
    async fn named_voice_for(
        &self,
        recording_id: &str,
        speaker_label: i64,
    ) -> Result<Option<String>> {
        let row = sqlx::query(
            "SELECT named_voice_id FROM speaker_voiceprints \
             WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(recording_id)
        .bind(speaker_label)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(r.try_get::<Option<String>, _>("named_voice_id")?),
            None => Ok(None),
        }
    }

    /// Enroll a recording's speaker into the named-voice library under `name` —
    /// the implicit-enrollment path, called whenever a speaker is named. Finds or
    /// creates the named voice by case-insensitive name, links the capture to it,
    /// and recomputes the library entry's cached centroid. Returns the named-voice
    /// id, or `None` when there's no captured voiceprint to enroll (e.g. a
    /// cloud-diarized recording) or the name is blank.
    pub async fn enroll_speaker(
        &self,
        recording_id: &str,
        speaker_label: i64,
        name: &str,
    ) -> Result<Option<String>> {
        let name = name.trim();
        if name.is_empty()
            || self
                .speaker_voiceprint(recording_id, speaker_label)
                .await?
                .is_none()
        {
            return Ok(None);
        }
        // What this capture was enrolled under before (if anything), so a re-name
        // (e.g. correcting a wrong suggestion) recomputes the OLD voice too — else
        // it keeps the moved sample's stale centroid / inflated count (audit H2).
        let previous = self.named_voice_for(recording_id, speaker_label).await?;
        let id = self.find_or_create_named_voice(name).await?;
        sqlx::query(
            "UPDATE speaker_voiceprints SET named_voice_id = ? \
             WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(&id)
        .bind(recording_id)
        .bind(speaker_label)
        .execute(&self.pool)
        .await?;
        // Dismissals are name-agnostic: a speaker dismissed before the right
        // voice existed in the library left a row that would suppress every
        // future suggestion for this (recording, label). Naming it is an
        // explicit identification, so clear that row — once a matching voice
        // exists the speaker can be recognized/renamed again (audit M9).
        sqlx::query(
            "DELETE FROM dismissed_speaker_suggestions \
             WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(recording_id)
        .bind(speaker_label)
        .execute(&self.pool)
        .await?;
        self.recompute_named_centroid(&id).await?;
        if let Some(prev) = previous {
            if prev != id {
                self.recompute_named_centroid(&prev).await?;
            }
        }
        Ok(Some(id))
    }

    /// Un-enroll a recording's speaker from its named voice (keeps the raw
    /// capture; recomputes the formerly-linked voice). No-op when not enrolled.
    pub async fn unenroll_speaker(&self, recording_id: &str, speaker_label: i64) -> Result<()> {
        if let Some(nid) = self.named_voice_for(recording_id, speaker_label).await? {
            sqlx::query(
                "UPDATE speaker_voiceprints SET named_voice_id = NULL \
                 WHERE recording_id = ? AND speaker_label = ?",
            )
            .bind(recording_id)
            .bind(speaker_label)
            .execute(&self.pool)
            .await?;
            self.recompute_named_centroid(&nid).await?;
        }
        Ok(())
    }

    /// Find a named voice by case-insensitive name, creating an empty one (no
    /// samples yet) if none matches. Returns its id.
    async fn find_or_create_named_voice(&self, name: &str) -> Result<String> {
        // Atomic find-or-create: the INSERT is a single (serialized) write that
        // fires only when no case-insensitive match exists, so two enrollments
        // racing under the same name can't create duplicate library entries
        // (audit M4). Then read back the existing-or-just-created id.
        let id = format!("nv_{}", RecordingId::new().as_str());
        // A *forgotten* (soft-deleted) voice with this name does not count as a
        // match — re-using a forgotten name creates a fresh live voice rather than
        // silently reviving a tombstoned one (undo is the explicit revive path).
        sqlx::query(
            "INSERT INTO named_voiceprints (id, name, centroid, samples) \
             SELECT ?, ?, '[]', 0 \
             WHERE NOT EXISTS (SELECT 1 FROM named_voiceprints \
                 WHERE name = ? COLLATE NOCASE AND deleted_at IS NULL)",
        )
        .bind(&id)
        .bind(name)
        .bind(name)
        .execute(&self.pool)
        .await?;
        let resolved: String = sqlx::query_scalar(
            "SELECT id FROM named_voiceprints \
             WHERE name = ? COLLATE NOCASE AND deleted_at IS NULL LIMIT 1",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        Ok(resolved)
    }

    /// Drop clear outliers from a named voice's linked captures before averaging.
    ///
    /// Each sample is `(centroid, duration_weight)`; outliers are judged purely on
    /// centroid *geometry* (a long sample of the wrong speaker is still the wrong
    /// speaker), and each survivor keeps its weight so the caller can take a
    /// duration-weighted mean over what's left (roadmap V4).
    ///
    /// With `< 4` samples there's too little signal to judge, so the input is
    /// returned unchanged. At `>= 4`, a provisional (unweighted) mean is taken,
    /// then any capture whose cosine to that mean is below either a hard floor
    /// (`0.2` — almost certainly a different speaker) or `mean - 2*stddev` (a
    /// statistical outlier) is removed. If pruning would drop *everything* (a
    /// degenerate cutoff, or a provisional mean that can't be computed — e.g.
    /// mixed dimensions), the originals are kept so the voice never silently
    /// empties.
    fn drop_centroid_outliers(samples: Vec<(Vec<f32>, f64)>) -> Vec<(Vec<f32>, f64)> {
        const MIN_SAMPLES_TO_PRUNE: usize = 4;
        const HARD_FLOOR: f32 = 0.2;

        if samples.len() < MIN_SAMPLES_TO_PRUNE {
            return samples;
        }
        // The provisional mean used for outlier detection is UNWEIGHTED — duration
        // must not decide who counts as an outlier, only how much a survivor
        // contributes to the final template.
        let centroids: Vec<Vec<f32>> = samples.iter().map(|(c, _)| c.clone()).collect();
        let provisional = match crate::voiceprint::mean_centroid(&centroids) {
            Some(m) => m,
            None => return samples, // mixed dims etc. — can't judge, keep all
        };
        let sims: Vec<f32> = samples
            .iter()
            .map(|(c, _)| crate::voiceprint::cosine_similarity(c, &provisional))
            .collect();
        let n = sims.len() as f32;
        let mean: f32 = sims.iter().sum::<f32>() / n;
        let var: f32 = sims.iter().map(|s| (s - mean) * (s - mean)).sum::<f32>() / n;
        let cutoff = (mean - 2.0 * var.sqrt()).max(HARD_FLOOR);

        let kept: Vec<(Vec<f32>, f64)> = samples
            .iter()
            .zip(sims.iter())
            .filter(|(_, &sim)| sim >= cutoff)
            .map(|((c, w), _)| (c.clone(), *w))
            .collect();
        // A degenerate cutoff (every sample identical → var 0, and float jitter
        // drops the whole set) must not empty the voice — fall back to the full
        // sample set in that case.
        if kept.is_empty() {
            return samples;
        }
        kept
    }

    /// Recompute a named voice's cached centroid + sample count from its linked
    /// captures: the **duration-weighted** L2-normalized mean (roadmap V4), taken
    /// over the survivors of [`Self::drop_centroid_outliers`]. Weighting is
    /// applied *after* outlier rejection, so a long sample only counts more once
    /// it's already been judged a genuine member of the cluster — it can't drag in
    /// a wrong-speaker capture just by being lengthy. Legacy captures with
    /// `duration_ms = 0` fall back to equal weighting, so a library built before
    /// this feature recomputes to the identical centroid until new, duration-
    /// bearing captures arrive. A voice with no remaining captures gets an empty
    /// centroid and zero samples (it never matches, but the entry stays until
    /// explicitly forgotten).
    async fn recompute_named_centroid(&self, named_voice_id: &str) -> Result<()> {
        let rows = sqlx::query(
            "SELECT centroid, duration_ms FROM speaker_voiceprints WHERE named_voice_id = ?",
        )
        .bind(named_voice_id)
        .fetch_all(&self.pool)
        .await?;
        let mut samples: Vec<(Vec<f32>, f64)> = Vec::with_capacity(rows.len());
        for r in rows {
            let centroid = serde_json::from_str::<Vec<f32>>(&r.try_get::<String, _>("centroid")?)?;
            let duration_ms: i64 = r.try_get("duration_ms")?;
            samples.push((centroid, duration_ms as f64));
        }
        // With enough captures, prune clear outliers before the final mean so one
        // mis-assigned sample (a wrong-speaker capture named into this voice)
        // can't drag the template off the real speaker (audit M7). Below the
        // threshold every sample counts — too few to tell signal from noise.
        let kept = Self::drop_centroid_outliers(samples);
        let mean = crate::voiceprint::weighted_mean_centroid(&kept).unwrap_or_default();
        let json = serde_json::to_string(&mean)?;
        sqlx::query(
            "UPDATE named_voiceprints SET centroid = ?, samples = ?, updated_at = datetime('now') \
             WHERE id = ?",
        )
        .bind(&json)
        .bind(kept.len() as i64)
        .bind(named_voice_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The named-voice library (id, name, sample count), most-recently-updated
    /// first — for the Speaker Library manager.
    pub async fn list_named_voices(&self) -> Result<Vec<NamedVoice>> {
        let rows = sqlx::query(
            "SELECT id, name, samples FROM named_voiceprints \
             WHERE deleted_at IS NULL \
             ORDER BY updated_at DESC, name COLLATE NOCASE ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(NamedVoice {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    samples: r.try_get::<i64, _>("samples")? as u32,
                })
            })
            .collect()
    }

    /// Match a probe centroid against the named-voice library, returning the best
    /// `(NamedVoice, score)` at or above `threshold`. Voices with no samples
    /// (empty centroid) never match. Used by recognition to suggest a name.
    pub async fn recognize_voice(
        &self,
        probe: &[f32],
        threshold: f32,
    ) -> Result<Option<(NamedVoice, f32)>> {
        let rows = sqlx::query(
            "SELECT id, name, samples, centroid FROM named_voiceprints \
             WHERE deleted_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut voices: Vec<NamedVoice> = Vec::with_capacity(rows.len());
        let mut centroids: Vec<Vec<f32>> = Vec::with_capacity(rows.len());
        for r in rows {
            let samples: i64 = r.try_get("samples")?;
            if samples <= 0 {
                continue;
            }
            let id: String = r.try_get("id")?;
            let centroid = serde_json::from_str::<Vec<f32>>(&r.try_get::<String, _>("centroid")?)?;
            // A centroid whose dimension differs from the probe came from a
            // different embedding model — cosine would silently return 0.0, so
            // such a library would go quietly unmatched. Skip it with a signal
            // instead (audit L).
            if centroid.len() != probe.len() {
                tracing::warn!(
                    id = %id,
                    dim = centroid.len(),
                    probe_dim = probe.len(),
                    "skipping named voice: centroid dimension mismatch (cross-model library)"
                );
                continue;
            }
            voices.push(NamedVoice {
                id,
                name: r.try_get("name")?,
                samples: samples as u32,
            });
            centroids.push(centroid);
        }
        Ok(crate::voiceprint::best_match(probe, &centroids, threshold)
            .map(|(i, score)| (voices[i].clone(), score)))
    }

    /// Rename a named voice (no-op on a blank name or unknown id).
    pub async fn rename_named_voice(&self, id: &str, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "UPDATE named_voiceprints SET name = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(name)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Merge `from_id` into `into_id`: re-point all of `from`'s captures onto
    /// `into`, recompute `into`'s centroid, and delete `from`. Returns whether a
    /// merge happened (both ids exist and differ).
    pub async fn merge_named_voices(&self, from_id: &str, into_id: &str) -> Result<bool> {
        if from_id == into_id {
            return Ok(false);
        }
        let both: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM named_voiceprints WHERE id IN (?, ?)")
                .bind(from_id)
                .bind(into_id)
                .fetch_one(&self.pool)
                .await?;
        if both < 2 {
            return Ok(false);
        }
        sqlx::query("UPDATE speaker_voiceprints SET named_voice_id = ? WHERE named_voice_id = ?")
            .bind(into_id)
            .bind(from_id)
            .execute(&self.pool)
            .await?;
        // Drop any undo-log rows keyed by the absorbed voice — the hard delete below
        // has no FK cascade, so these would otherwise orphan.
        sqlx::query("DELETE FROM forgotten_voiceprint_links WHERE named_voice_id = ?")
            .bind(from_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM named_voiceprints WHERE id = ?")
            .bind(from_id)
            .execute(&self.pool)
            .await?;
        self.recompute_named_centroid(into_id).await?;
        Ok(true)
    }

    /// Forget a named voice — reversibly (roadmap V5). The library entry is
    /// soft-deleted (`deleted_at` stamped, not dropped), its captures are unlinked
    /// (the raw per-recording voiceprints stay), and which captures were unlinked
    /// is recorded in the undo log so [`Self::undo_forget`] can re-link exactly
    /// those rows. A tombstoned voice is invisible to listing and recognition just
    /// like the old hard delete, but recoverable. Returns whether a live row was
    /// forgotten (false for an unknown or already-forgotten id). Idempotent: a
    /// second forget of the same id is a no-op.
    pub async fn forget_named_voice(&self, id: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        // Only a LIVE voice can be forgotten; this both guards idempotency and
        // gives us the rows_affected to report.
        let stamped = sqlx::query(
            "UPDATE named_voiceprints SET deleted_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if stamped.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(false);
        }
        // Snapshot which captures are about to be unlinked, so undo can restore the
        // exact set (a stale log from a prior forget of this id is cleared first).
        sqlx::query("DELETE FROM forgotten_voiceprint_links WHERE named_voice_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO forgotten_voiceprint_links (named_voice_id, recording_id, speaker_label) \
             SELECT named_voice_id, recording_id, speaker_label FROM speaker_voiceprints \
             WHERE named_voice_id = ?",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE speaker_voiceprints SET named_voice_id = NULL WHERE named_voice_id = ?",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Undo a [`Self::forget_named_voice`]: clear the tombstone, re-link the
    /// captures the forget unlinked (from the undo log), recompute the cached
    /// centroid, and clear the log. Returns whether a forgotten voice was restored
    /// (false for an unknown or not-currently-forgotten id). Idempotent on a
    /// live id.
    pub async fn undo_forget(&self, id: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let revived = sqlx::query(
            "UPDATE named_voiceprints SET deleted_at = NULL \
             WHERE id = ? AND deleted_at IS NOT NULL",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if revived.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(false);
        }
        // Re-link only captures still currently unlinked — a capture re-named onto
        // a different voice since the forget keeps its newer assignment (don't
        // clobber a deliberate re-enrollment).
        sqlx::query(
            "UPDATE speaker_voiceprints SET named_voice_id = ? \
             WHERE named_voice_id IS NULL AND (recording_id, speaker_label) IN \
                 (SELECT recording_id, speaker_label FROM forgotten_voiceprint_links \
                  WHERE named_voice_id = ?)",
        )
        .bind(id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM forgotten_voiceprint_links WHERE named_voice_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        // Re-derive the cached centroid + count from the re-linked captures.
        self.recompute_named_centroid(id).await?;
        Ok(true)
    }

    // ---- Name propagation — back-fill a name onto past recordings (V5) ----

    /// Find the *unnamed* speakers in OTHER recordings whose voiceprint matches a
    /// named voice — the back-fill candidates for naming propagation (V5).
    ///
    /// Scans every `speaker_voiceprints` row with `named_voice_id IS NULL` (a
    /// captured-but-never-named speaker), in any recording other than ones already
    /// linked to this voice, and scores its centroid against the named voice's
    /// cached centroid with the *same scorer the recognizer uses* — raw cosine
    /// under [`ScoreNorm::Off`](crate::voiceprint::ScoreNorm::Off), or the z-score
    /// under `s_norm`/`as_norm`, cohort = the live named-voice library. A row is a
    /// candidate iff it clears `threshold` (interpret `threshold` on the same scale
    /// the chosen `mode` does, exactly as `recognize_speakers_for`). Already-named
    /// speakers are never candidates (only `named_voice_id IS NULL` rows are
    /// scanned), so propagation can only ADD a name, never overwrite one. Results
    /// are ordered by score, highest first.
    ///
    /// Returns empty when the voice is unknown, forgotten, or has no centroid yet.
    pub async fn propagation_candidates(
        &self,
        named_voice_id: &str,
        threshold: f32,
        mode: crate::voiceprint::ScoreNorm,
    ) -> Result<Vec<PropagationCandidate>> {
        // The live library: the cohort for normalization AND the source of the
        // target centroid. A forgotten / sample-less voice isn't here, so it yields
        // no candidates.
        let library = self.named_voice_centroids().await?;
        let target_idx = match library.iter().position(|(v, _)| v.id == named_voice_id) {
            Some(i) => i,
            None => return Ok(Vec::new()),
        };
        let cohort: Vec<Vec<f32>> = library.iter().map(|(_, c)| c.clone()).collect();

        // Every UNNAMED capture: not enrolled (`named_voice_id IS NULL`) AND with
        // no display name (`speaker_names`) — a speaker can carry a display name
        // without being enrolled (e.g. the pipeline's "You" default, or a name set
        // on a cloud-diarized recording with no voiceprint), and propagation must
        // never overwrite such a name. Also exclude recordings already enrolled
        // under THIS voice, keeping a recording out of its own back-fill.
        let rows = sqlx::query(
            "SELECT sv.recording_id AS recording_id, sv.speaker_label AS speaker_label, \
                    sv.centroid AS centroid \
             FROM speaker_voiceprints sv \
             WHERE sv.named_voice_id IS NULL \
               AND NOT EXISTS (SELECT 1 FROM speaker_names sn \
                     WHERE sn.recording_id = sv.recording_id \
                       AND sn.speaker_label = sv.speaker_label) \
               AND sv.recording_id NOT IN \
                 (SELECT recording_id FROM speaker_voiceprints WHERE named_voice_id = ?)",
        )
        .bind(named_voice_id)
        .fetch_all(&self.pool)
        .await?;

        let mut out: Vec<PropagationCandidate> = Vec::new();
        for r in rows {
            let centroid = serde_json::from_str::<Vec<f32>>(&r.try_get::<String, _>("centroid")?)?;
            // A dimension mismatch (cross-model capture) scores cosine 0.0 and
            // won't clear any sane threshold — same skip the recognizer makes.
            let score = crate::voiceprint::normalized_score(&centroid, &cohort, target_idx, mode);
            if score >= threshold {
                let recording_id: String = r.try_get("recording_id")?;
                let speaker_label: i64 = r.try_get("speaker_label")?;
                let rid = match RecordingId::parse(recording_id) {
                    Some(rid) => rid,
                    None => continue, // a malformed id can't be a real recording
                };
                out.push(PropagationCandidate {
                    recording_id: rid,
                    speaker_label,
                    score,
                });
            }
        }
        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(out)
    }

    /// Back-fill a named voice onto the given `(recording_id, speaker_label)`
    /// targets (V5) — the apply half of propagation.
    ///
    /// For each target it does exactly what a normal naming does: set the
    /// per-recording display name (`speaker_names`, the same row
    /// [`Self::set_speaker_name`] writes — the transcript's `[Speaker N]` markers
    /// are mapped to it at display/export time, never rewritten in place) and
    /// enroll the capture into the library so the voice gets stronger. The voice's
    /// own name is read from the library by id.
    ///
    /// Safety + idempotency: a target whose speaker is ALREADY named is skipped (we
    /// never overwrite a name), as is a target with no captured voiceprint or one
    /// already enrolled under this voice. Re-running with the same targets does no
    /// duplicate work. Returns how many targets were actually back-filled.
    ///
    /// Best-effort per target: the voice must exist and be live (a forgotten voice
    /// back-fills nothing).
    pub async fn apply_propagation(
        &self,
        named_voice_id: &str,
        targets: &[(RecordingId, i64)],
    ) -> Result<usize> {
        // Resolve the name from the LIVE library; a forgotten/unknown voice can't
        // be propagated.
        let name: Option<String> = sqlx::query_scalar(
            "SELECT name FROM named_voiceprints WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(named_voice_id)
        .fetch_optional(&self.pool)
        .await?;
        let name = match name {
            Some(n) => n,
            None => return Ok(0),
        };

        let mut applied = 0usize;
        for (recording_id, speaker_label) in targets {
            // Never overwrite an existing name — only back-fill an unnamed speaker.
            let already_named: Option<String> = sqlx::query_scalar(
                "SELECT name FROM speaker_names WHERE recording_id = ? AND speaker_label = ?",
            )
            .bind(recording_id.as_str())
            .bind(speaker_label)
            .fetch_optional(&self.pool)
            .await?;
            if already_named.is_some() {
                continue;
            }
            // Skip a capture already enrolled under THIS voice (idempotent re-run).
            if self
                .named_voice_for(recording_id.as_str(), *speaker_label)
                .await?
                .as_deref()
                == Some(named_voice_id)
            {
                continue;
            }
            // Apply the display name (same write as set_speaker_name) and enroll the
            // capture. enroll_speaker is a no-op when no voiceprint was captured.
            self.set_speaker_name(recording_id, *speaker_label, &name)
                .await?;
            // Don't `?` the enroll: a failure AFTER the name was written would leave a
            // display name with no enrollment. Match so we can roll the name back.
            match self
                .enroll_speaker(recording_id.as_str(), *speaker_label, &name)
                .await
            {
                Ok(Some(_)) => applied += 1,
                Ok(None) => {
                    // No voiceprint to enroll — undo the name we just wrote so a target
                    // with no capture isn't half-applied (display-named but unenrolled).
                    self.set_speaker_name(recording_id, *speaker_label, "")
                        .await?;
                }
                Err(e) => {
                    // Enrollment failed — roll back the name we just wrote, then
                    // propagate the error (best-effort undo; the enroll error wins).
                    let _ = self
                        .set_speaker_name(recording_id, *speaker_label, "")
                        .await;
                    return Err(e);
                }
            }
        }
        Ok(applied)
    }

    /// All captured voiceprints for a recording, as `(speaker_label, centroid)`.
    pub async fn speaker_voiceprints_for(
        &self,
        recording_id: &str,
    ) -> Result<Vec<(i64, Vec<f32>)>> {
        let rows = sqlx::query(
            "SELECT speaker_label, centroid FROM speaker_voiceprints \
             WHERE recording_id = ? ORDER BY speaker_label",
        )
        .bind(recording_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let label: i64 = r.try_get("speaker_label")?;
            let centroid = serde_json::from_str::<Vec<f32>>(&r.try_get::<String, _>("centroid")?)?;
            out.push((label, centroid));
        }
        Ok(out)
    }

    /// Mark a recognized-speaker suggestion dismissed so it isn't offered again.
    pub async fn dismiss_speaker_suggestion(
        &self,
        recording_id: &str,
        speaker_label: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO dismissed_speaker_suggestions (recording_id, speaker_label) \
             VALUES (?, ?)",
        )
        .bind(recording_id)
        .bind(speaker_label)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One column of i64s from a recording-scoped table, as a set.
    async fn label_set(
        &self,
        query: &str,
        recording_id: &str,
    ) -> Result<std::collections::HashSet<i64>> {
        let rows = sqlx::query(query)
            .bind(recording_id)
            .fetch_all(&self.pool)
            .await?;
        let mut set = std::collections::HashSet::with_capacity(rows.len());
        for r in rows {
            set.insert(r.try_get::<i64, _>("speaker_label")?);
        }
        Ok(set)
    }

    /// The named-voice library as `(NamedVoice, centroid)` pairs, skipping
    /// empty entries (no samples). Used by recognition to score every captured
    /// speaker against the whole library at once.
    async fn named_voice_centroids(&self) -> Result<Vec<(NamedVoice, Vec<f32>)>> {
        let rows = sqlx::query(
            "SELECT id, name, samples, centroid FROM named_voiceprints \
             WHERE deleted_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let samples: i64 = r.try_get("samples")?;
            if samples <= 0 {
                continue;
            }
            let centroid = serde_json::from_str::<Vec<f32>>(&r.try_get::<String, _>("centroid")?)?;
            out.push((
                NamedVoice {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    samples: samples as u32,
                },
                centroid,
            ));
        }
        Ok(out)
    }

    /// On-demand named-speaker recognition for a recording (#9): each captured
    /// speaker that has NO name yet and hasn't been dismissed is matched against
    /// the named-voice library, and the surviving suggestions are returned.
    /// Reflects the live library, so a voice named *after* this recording was
    /// transcribed is still suggested here.
    ///
    /// Assignment is **one-to-one**: the full captured-speaker × named-voice
    /// cosine matrix is scored once, then the best pairs are taken in descending
    /// score so no two speakers in the same recording can be handed the same
    /// name (audit H2). A pair is only emitted when it clears `threshold` AND
    /// beats that speaker's own second-best candidate by `MARGIN` — an
    /// ambiguous speaker (two library voices nearly tied) is left unknown rather
    /// than guessed. The result holds at most one suggestion per captured speaker
    /// and per name, ordered by `speaker_label`.
    pub async fn recognize_speakers_for(
        &self,
        recording_id: &str,
        threshold: f32,
        mode: crate::voiceprint::ScoreNorm,
    ) -> Result<Vec<SpeakerSuggestion>> {
        let captured = self.speaker_voiceprints_for(recording_id).await?;
        if captured.is_empty() {
            return Ok(Vec::new());
        }
        let named = self
            .label_set(
                "SELECT speaker_label FROM speaker_names WHERE recording_id = ?",
                recording_id,
            )
            .await?;
        let dismissed = self
            .label_set(
                "SELECT speaker_label FROM dismissed_speaker_suggestions WHERE recording_id = ?",
                recording_id,
            )
            .await?;
        // Only speakers still eligible for a suggestion (un-named, un-dismissed).
        let probes: Vec<(i64, Vec<f32>)> = captured
            .into_iter()
            .filter(|(label, _)| !named.contains(label) && !dismissed.contains(label))
            .collect();
        if probes.is_empty() {
            return Ok(Vec::new());
        }
        let library = self.named_voice_centroids().await?;
        Ok(Self::assign_speakers(&probes, &library, threshold, mode))
    }

    /// Margin a winning match must beat the speaker's second-best by — below
    /// this gap the two candidates are too close to call, so no name is offered.
    const MARGIN: f32 = 0.05;

    /// One-to-one greedy assignment of captured speakers to named voices.
    ///
    /// Builds the score matrix, then repeatedly takes the highest remaining
    /// `(speaker, voice)` cell whose speaker and voice are both still free, the
    /// score clears `threshold`, and the score beats that speaker's *second-best
    /// over the whole library* by `MARGIN`. Each speaker and each named
    /// voice is used at most once. Output is sorted by `speaker_label` for a
    /// stable suggestion order.
    ///
    /// `mode` selects the scorer (V2): [`ScoreNorm::Off`](crate::voiceprint::ScoreNorm::Off)
    /// uses raw cosine and `threshold` is the cosine bar (unchanged behavior);
    /// `s_norm`/`as_norm` z-score each probe against the rest of the *library*
    /// (the cohort) and `threshold` is then a z-score bar. The library is the
    /// cohort for both probe-side and (AS-norm) target-side normalization.
    fn assign_speakers(
        probes: &[(i64, Vec<f32>)],
        library: &[(NamedVoice, Vec<f32>)],
        threshold: f32,
        mode: crate::voiceprint::ScoreNorm,
    ) -> Vec<SpeakerSuggestion> {
        if library.is_empty() {
            return Vec::new();
        }
        // Cohort = the library centroids; normalize each probe against the *other*
        // library voices. With ScoreNorm::Off this is exactly the raw cosine, so
        // the default path is unchanged. A dimension mismatch yields cosine 0.0
        // (treated as no signal), matching `recognize_voice`'s skip; the margin
        // test below keeps such a row from ever winning.
        let cohort: Vec<Vec<f32>> = library.iter().map(|(_, c)| c.clone()).collect();
        let scores: Vec<Vec<f32>> = probes
            .iter()
            .map(|(_, probe)| {
                (0..cohort.len())
                    .map(|ti| crate::voiceprint::normalized_score(probe, &cohort, ti, mode))
                    .collect()
            })
            .collect();

        // Each speaker's second-best score across the whole library, used for the
        // ambiguity margin even after a voice is claimed by another speaker.
        let second_best: Vec<f32> = scores
            .iter()
            .map(|row| {
                let mut top2 = [f32::NEG_INFINITY, f32::NEG_INFINITY];
                for &s in row {
                    if s > top2[0] {
                        top2[1] = top2[0];
                        top2[0] = s;
                    } else if s > top2[1] {
                        top2[1] = s;
                    }
                }
                if top2[1].is_finite() {
                    top2[1]
                } else {
                    f32::NEG_INFINITY // only one candidate → no rival to clear
                }
            })
            .collect();

        let mut speaker_taken = vec![false; probes.len()];
        let mut voice_taken = vec![false; library.len()];
        let mut out: Vec<SpeakerSuggestion> = Vec::new();

        // Greedy: pick the globally-highest free cell each round until none qualify.
        loop {
            let mut best: Option<(usize, usize, f32)> = None;
            for (si, row) in scores.iter().enumerate() {
                if speaker_taken[si] {
                    continue;
                }
                for (vi, &score) in row.iter().enumerate() {
                    if voice_taken[vi] || score < threshold {
                        continue;
                    }
                    // Clear the speaker's own runner-up by a margin, else it's
                    // ambiguous — leave this speaker unknown.
                    let runner_up = second_best[si];
                    if runner_up.is_finite() && score < runner_up + Self::MARGIN {
                        continue;
                    }
                    if best.is_none_or(|(_, _, b)| score > b) {
                        best = Some((si, vi, score));
                    }
                }
            }
            match best {
                Some((si, vi, score)) => {
                    speaker_taken[si] = true;
                    voice_taken[vi] = true;
                    out.push(SpeakerSuggestion {
                        speaker_label: probes[si].0,
                        name: library[vi].0.name.clone(),
                        named_voice_id: library[vi].0.id.clone(),
                        score,
                    });
                }
                None => break,
            }
        }
        out.sort_by_key(|s| s.speaker_label);
        out
    }

    /// Replace a recording's machine transcript segments with a fresh set.
    ///
    /// Called by the pipeline after every transcribe/retranscribe — segments
    /// always describe the *current* machine output, so the old rows are
    /// dropped first (in the same transaction, so a crash can't leave a
    /// half-replaced timeline). An empty slice simply clears them (e.g. a
    /// provider that returns no timing data).
    pub async fn replace_segments(
        &self,
        recording_id: &RecordingId,
        segments: &[TranscriptSegment],
    ) -> Result<()> {
        self.replace_segments_variant(recording_id, "raw", segments)
            .await
    }

    /// Replace one timing variant of a recording's segments, leaving the other
    /// variant intact (TL-CONSISTENCY). `"raw"` is the machine-truth timeline (what
    /// [`replace_segments`](Self::replace_segments) writes); `"cleaned"` is the
    /// timeline re-aligned to the post-cleanup transcript. Same
    /// replace-on-(re)transcribe semantics, scoped to `variant`.
    ///
    /// NOTE: the U1 speaker-correction ops (`reassign_segment`/merge/split) edit
    /// `transcript_segments` by `(recording_id, idx)` without a variant filter;
    /// before `"cleaned"` rows exist that is harmless (only `"raw"` rows are
    /// present), but those ops must be scoped to `"raw"` once the cleaned re-flow
    /// is wired.
    pub async fn replace_segments_variant(
        &self,
        recording_id: &RecordingId,
        variant: &str,
        segments: &[TranscriptSegment],
    ) -> Result<()> {
        let table = segments_table(variant);
        let mut tx = self.pool.begin().await?;
        sqlx::query(&format!("DELETE FROM {table} WHERE recording_id = ?"))
            .bind(recording_id.as_str())
            .execute(&mut *tx)
            .await?;
        for (idx, seg) in segments.iter().enumerate() {
            sqlx::query(&format!(
                "INSERT INTO {table} (recording_id, idx, start_ms, end_ms, text, speaker) \
                 VALUES (?, ?, ?, ?, ?, ?)"
            ))
            .bind(recording_id.as_str())
            .bind(idx as i64)
            .bind(seg.start_ms)
            .bind(seg.end_ms)
            .bind(&seg.text)
            .bind(&seg.speaker)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// A recording's machine transcript segments in timeline order. Empty when
    /// the recording predates segment capture or its provider returned no
    /// timing data — callers must treat "no segments" as a normal state, not
    /// an error.
    pub async fn segments_for(&self, recording_id: &RecordingId) -> Result<Vec<TranscriptSegment>> {
        self.segments_for_variant(recording_id, "raw").await
    }

    /// A recording's segments for one timing `variant` (`"raw"` or `"cleaned"`),
    /// in timeline order. Empty when that variant has no rows (a recording with no
    /// cleanup has no `"cleaned"` timeline) — a normal state, not an error.
    pub async fn segments_for_variant(
        &self,
        recording_id: &RecordingId,
        variant: &str,
    ) -> Result<Vec<TranscriptSegment>> {
        let table = segments_table(variant);
        let rows = sqlx::query(&format!(
            "SELECT start_ms, end_ms, text, speaker FROM {table} \
             WHERE recording_id = ? ORDER BY idx"
        ))
        .bind(recording_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(TranscriptSegment {
                    start_ms: r.try_get("start_ms")?,
                    end_ms: r.try_get("end_ms")?,
                    text: r.try_get("text")?,
                    speaker: r.try_get("speaker")?,
                })
            })
            .collect()
    }

    // ── Compounding transcript versions (PB-COMPOUND) ───────────────────────
    //
    // A compounding recipe chains Transform steps (each rewrites the running
    // transcript); these methods record every step's output so the chain is
    // inspectable + revertible. Replaced wholesale per (re)transcription, like
    // segments. The executor + the Compare-versions IPC/UI wire onto these.

    /// Replace all transcript versions for a recording (wholesale, single tx).
    /// Pass them in `idx` order (`0` = raw ASR). An empty slice clears them.
    pub async fn replace_transcript_versions(
        &self,
        recording_id: &RecordingId,
        versions: &[TranscriptVersion],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM transcript_versions WHERE recording_id = ?")
            .bind(recording_id.as_str())
            .execute(&mut *tx)
            .await?;
        for v in versions {
            sqlx::query(
                "INSERT INTO transcript_versions \
                 (recording_id, idx, step_id, label, model, text) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(recording_id.as_str())
            .bind(v.idx)
            .bind(&v.step_id)
            .bind(&v.label)
            .bind(&v.model)
            .bind(&v.text)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// A recording's transcript versions in step order. Empty for recordings that
    /// predate compounding — callers treat "no versions" as a normal state.
    pub async fn transcript_versions_for(
        &self,
        recording_id: &RecordingId,
    ) -> Result<Vec<TranscriptVersion>> {
        let rows = sqlx::query(
            "SELECT idx, step_id, label, model, text FROM transcript_versions \
             WHERE recording_id = ? ORDER BY idx",
        )
        .bind(recording_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(TranscriptVersion {
                    idx: r.try_get("idx")?,
                    step_id: r.try_get("step_id")?,
                    label: r.try_get("label")?,
                    model: r.try_get("model")?,
                    text: r.try_get("text")?,
                })
            })
            .collect()
    }

    /// One transcript version by step `idx`, if present.
    pub async fn transcript_version(
        &self,
        recording_id: &RecordingId,
        idx: i64,
    ) -> Result<Option<TranscriptVersion>> {
        let row = sqlx::query(
            "SELECT idx, step_id, label, model, text FROM transcript_versions \
             WHERE recording_id = ? AND idx = ?",
        )
        .bind(recording_id.as_str())
        .bind(idx)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            Ok(TranscriptVersion {
                idx: r.try_get("idx")?,
                step_id: r.try_get("step_id")?,
                label: r.try_get("label")?,
                model: r.try_get("model")?,
                text: r.try_get("text")?,
            })
        })
        .transpose()
    }

    // ── In-recording speaker correction (U1) ───────────────────────────────
    //
    // Let the user fix the diarizer's per-segment speaker assignments after the
    // fact: reassign one segment, merge two speakers into one, or split some
    // segments off into a fresh speaker. `transcript_segments.speaker` is the
    // authoritative store (the timeline / Synced views re-derive from it). The
    // prose `transcript` text's `[Speaker N]:` markers are a SEPARATE display
    // source (the detail prose view and the rename modal read them), so every
    // op that changes which segment belongs to which speaker also rebuilds those
    // markers from the updated segments in the SAME transaction — otherwise the
    // prose view would disagree with the timeline. Labels are the 1-based
    // integer `[Speaker N]` index that also keys `speaker_names` /
    // `speaker_voiceprints`; in `transcript_segments`/`transcript_words` they are
    // stored as that integer's text form ("1", "2", …).

    /// Reassign one segment to a different speaker label.
    ///
    /// Sets `transcript_segments[idx].speaker` to `new_label` (and the matching
    /// `transcript_words` rows, so the word layer agrees), then rebuilds the
    /// prose transcript's `[Speaker N]:` markers from the updated segments. A
    /// brand-new `new_label` simply starts existing — no name or voiceprint is
    /// created for it. Errors with [`Error::NotFound`](crate::error::Error) if no
    /// segment has that `idx` (no write happens). `new_label` must be ≥ 1.
    pub async fn reassign_segment(
        &self,
        recording_id: &RecordingId,
        idx: i64,
        new_label: i64,
    ) -> Result<()> {
        if new_label < 1 {
            return Err(crate::error::Error::Internal(format!(
                "invalid speaker label {new_label} (must be >= 1)"
            )));
        }
        let mut tx = self.pool.begin().await?;
        let label_text = new_label.to_string();
        // Match the segment by idx; capture its span so the word layer can be
        // repointed to the same speaker over the same time window.
        let span = sqlx::query(
            "SELECT start_ms, end_ms FROM transcript_segments WHERE recording_id = ? AND idx = ?",
        )
        .bind(recording_id.as_str())
        .bind(idx)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(span) = span else {
            return Err(crate::error::Error::NotFound {
                id: format!("segment {idx} of recording {}", recording_id.as_str()),
            });
        };
        let (start_ms, end_ms): (i64, i64) = (span.try_get("start_ms")?, span.try_get("end_ms")?);
        sqlx::query(
            "UPDATE transcript_segments SET speaker = ? WHERE recording_id = ? AND idx = ?",
        )
        .bind(&label_text)
        .bind(recording_id.as_str())
        .bind(idx)
        .execute(&mut *tx)
        .await?;
        // Keep the per-word layer in step: words inside the segment's span get
        // the same new label. Words carry no idx tie to a segment, so the span
        // is the join (the diarizer builds both from one attribution, so a
        // word's span lies within its segment's).
        sqlx::query(
            "UPDATE transcript_words SET speaker = ? \
             WHERE recording_id = ? AND start_ms >= ? AND start_ms < ?",
        )
        .bind(&label_text)
        .bind(recording_id.as_str())
        .bind(start_ms)
        .bind(end_ms)
        .execute(&mut *tx)
        .await?;
        Self::rebuild_transcript_markers_tx(&mut tx, recording_id).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Merge `from_label` into `into_label`: every segment (and word) of `from`
    /// becomes `into`, the `from` label ceases to exist.
    ///
    /// Consistency, all in one transaction:
    /// - **Segments / words**: repoint `speaker = from` → `into`.
    /// - **Names**: keep `into`'s name; if `into` has none, adopt `from`'s. The
    ///   now-defunct `from` name row is deleted either way.
    /// - **Voiceprints**: the captured centroid is *per recording-label*, so a
    ///   merged speaker's two captures can't be averaged into one meaningful
    ///   centroid here (we'd need the raw frames). The simplest correct choice is
    ///   to **drop `from`'s capture row** (and recompute its formerly-linked named
    ///   voice so the library no longer counts it); `into` keeps its own capture.
    ///   A re-transcribe re-captures a fresh, correct centroid for the merged
    ///   label. This favours a clean library over a blended centroid built from
    ///   the wrong inputs.
    ///
    /// Errors with [`Error::NotFound`](crate::error::Error) when no segment
    /// carries `from_label` (nothing to merge → no write). `from`/`into` must be
    /// ≥ 1 and differ.
    pub async fn merge_speakers(
        &self,
        recording_id: &RecordingId,
        from_label: i64,
        into_label: i64,
    ) -> Result<()> {
        if from_label < 1 || into_label < 1 {
            return Err(crate::error::Error::Internal(format!(
                "invalid speaker labels (from={from_label}, into={into_label}; must be >= 1)"
            )));
        }
        if from_label == into_label {
            return Err(crate::error::Error::Internal(
                "cannot merge a speaker into itself".into(),
            ));
        }
        let rid = recording_id.as_str();
        let from_text = from_label.to_string();
        let into_text = into_label.to_string();

        // The named voice `from`'s capture was enrolled under (if any) must be
        // recomputed AFTER its row is gone, so it stops counting the dropped
        // sample. Read it before the transaction's writes.
        let from_named_voice = self.named_voice_for(rid, from_label).await?;

        let mut tx = self.pool.begin().await?;
        // Guard: `from` must actually appear, else this is a no-op the caller
        // should hear about as an error (not a silent success).
        let from_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM transcript_segments WHERE recording_id = ? AND speaker = ?",
        )
        .bind(rid)
        .bind(&from_text)
        .fetch_one(&mut *tx)
        .await?;
        if from_count == 0 {
            return Err(crate::error::Error::NotFound {
                id: format!("speaker {from_label} of recording {rid}"),
            });
        }
        // Segments + words: repoint from → into.
        sqlx::query(
            "UPDATE transcript_segments SET speaker = ? WHERE recording_id = ? AND speaker = ?",
        )
        .bind(&into_text)
        .bind(rid)
        .bind(&from_text)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE transcript_words SET speaker = ? WHERE recording_id = ? AND speaker = ?",
        )
        .bind(&into_text)
        .bind(rid)
        .bind(&from_text)
        .execute(&mut *tx)
        .await?;

        // Names: `into` keeps its own; adopt `from`'s only when `into` is unnamed.
        let into_named: Option<String> = sqlx::query_scalar(
            "SELECT name FROM speaker_names WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(rid)
        .bind(into_label)
        .fetch_optional(&mut *tx)
        .await?;
        let from_named: Option<String> = sqlx::query_scalar(
            "SELECT name FROM speaker_names WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(rid)
        .bind(from_label)
        .fetch_optional(&mut *tx)
        .await?;
        if into_named.is_none() {
            if let Some(name) = from_named {
                sqlx::query(
                    "INSERT INTO speaker_names (recording_id, speaker_label, name) \
                     VALUES (?, ?, ?) \
                     ON CONFLICT(recording_id, speaker_label) DO UPDATE SET name = excluded.name",
                )
                .bind(rid)
                .bind(into_label)
                .bind(&name)
                .execute(&mut *tx)
                .await?;
            }
        }
        // The `from` name row is now defunct — drop it.
        sqlx::query("DELETE FROM speaker_names WHERE recording_id = ? AND speaker_label = ?")
            .bind(rid)
            .bind(from_label)
            .execute(&mut *tx)
            .await?;

        // Voiceprints: drop `from`'s capture (documented choice — see doc above).
        // `into`'s capture is left untouched.
        sqlx::query("DELETE FROM speaker_voiceprints WHERE recording_id = ? AND speaker_label = ?")
            .bind(rid)
            .bind(from_label)
            .execute(&mut *tx)
            .await?;
        // Any dismissed-suggestion row for the now-gone label is dead weight.
        sqlx::query(
            "DELETE FROM dismissed_speaker_suggestions WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(rid)
        .bind(from_label)
        .execute(&mut *tx)
        .await?;

        Self::rebuild_transcript_markers_tx(&mut tx, recording_id).await?;
        tx.commit().await?;

        // Recompute the library entry `from` fed, now that its sample is gone, so
        // the cross-recording centroid/count no longer reflect the dropped row.
        if let Some(nid) = from_named_voice {
            self.recompute_named_centroid(&nid).await?;
        }
        Ok(())
    }

    /// Split: move `segment_idxs` from `label` onto a fresh `new_label`.
    ///
    /// The listed segments (and their words) become `new_label`; every other
    /// segment of `label` stays put. The new label has no name and no voiceprint
    /// until the user names / re-enrolls it. The prose markers are rebuilt from
    /// the updated segments. Errors with [`Error::NotFound`](crate::error::Error)
    /// if any listed idx is missing or doesn't currently carry `label` (no
    /// partial write). `label`/`new_label` must be ≥ 1 and differ, and the idx
    /// list must be non-empty.
    pub async fn split_speaker(
        &self,
        recording_id: &RecordingId,
        label: i64,
        segment_idxs: &[i64],
        new_label: i64,
    ) -> Result<()> {
        if label < 1 || new_label < 1 {
            return Err(crate::error::Error::Internal(format!(
                "invalid speaker labels (label={label}, new={new_label}; must be >= 1)"
            )));
        }
        if label == new_label {
            return Err(crate::error::Error::Internal(
                "split target label must differ from the source".into(),
            ));
        }
        if segment_idxs.is_empty() {
            return Err(crate::error::Error::Internal(
                "split requires at least one segment index".into(),
            ));
        }
        let rid = recording_id.as_str();
        let label_text = label.to_string();
        let new_text = new_label.to_string();

        let mut tx = self.pool.begin().await?;
        // Validate every idx first (must exist AND currently belong to `label`),
        // collecting spans so the word layer can be repointed. A single bad idx
        // aborts the whole op with no write (the transaction is rolled back).
        let mut spans: Vec<(i64, i64)> = Vec::with_capacity(segment_idxs.len());
        for &idx in segment_idxs {
            let row = sqlx::query(
                "SELECT start_ms, end_ms, speaker FROM transcript_segments \
                 WHERE recording_id = ? AND idx = ?",
            )
            .bind(rid)
            .bind(idx)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(row) = row else {
                return Err(crate::error::Error::NotFound {
                    id: format!("segment {idx} of recording {rid}"),
                });
            };
            let cur: Option<String> = row.try_get("speaker")?;
            if cur.as_deref() != Some(label_text.as_str()) {
                return Err(crate::error::Error::Internal(format!(
                    "segment {idx} is not currently speaker {label} (it is {})",
                    cur.as_deref().unwrap_or("unassigned")
                )));
            }
            spans.push((row.try_get("start_ms")?, row.try_get("end_ms")?));
        }
        for (&idx, (start_ms, end_ms)) in segment_idxs.iter().zip(&spans) {
            sqlx::query(
                "UPDATE transcript_segments SET speaker = ? WHERE recording_id = ? AND idx = ?",
            )
            .bind(&new_text)
            .bind(rid)
            .bind(idx)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE transcript_words SET speaker = ? \
                 WHERE recording_id = ? AND start_ms >= ? AND start_ms < ?",
            )
            .bind(&new_text)
            .bind(rid)
            .bind(*start_ms)
            .bind(*end_ms)
            .execute(&mut *tx)
            .await?;
        }
        Self::rebuild_transcript_markers_tx(&mut tx, recording_id).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Rebuild the prose `transcript` text's `[Speaker N]:` markers from the
    /// current `transcript_segments`, inside an open transaction.
    ///
    /// The prose view and the rename modal read speaker structure from the stored
    /// `transcript` text, while the timeline / Synced views re-derive from
    /// segments — so after a label edit the text must be re-rendered to agree.
    /// Only diarized transcripts (those that already carry `[Speaker N]:`
    /// markers) are rebuilt; a plain (un-diarized) transcript has no markers to
    /// keep consistent and is left untouched. Consecutive same-label segments are
    /// coalesced into one `[Speaker N]: <text>` turn, turns joined by `\n\n` —
    /// the same shape the diarizer emits. Segments with no speaker are skipped
    /// from marker emission (they can't appear in a diarized turn anyway).
    ///
    /// The rebuild uses each segment's stored `text` joined with a space, which
    /// reproduces the diarized turn text for the local/cloud paths that build
    /// both from one attribution. It deliberately does NOT touch
    /// `original_transcript`/`clean_transcript` (machine truth is preserved) and
    /// does NOT set `user_edited` (a label correction is not a transcript hand
    /// edit).
    async fn rebuild_transcript_markers_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        recording_id: &RecordingId,
    ) -> Result<()> {
        let current: Option<String> =
            sqlx::query_scalar("SELECT transcript FROM recordings WHERE id = ?")
                .bind(recording_id.as_str())
                .fetch_optional(&mut **tx)
                .await?
                .flatten();
        let Some(current) = current else {
            return Ok(()); // No transcript row/text — nothing to keep consistent.
        };
        // Only diarized prose carries the markers we own; leave plain text alone.
        if !current.contains("[Speaker ") {
            return Ok(());
        }
        let rows = sqlx::query(
            "SELECT text, speaker FROM transcript_segments WHERE recording_id = ? ORDER BY idx",
        )
        .bind(recording_id.as_str())
        .fetch_all(&mut **tx)
        .await?;
        let mut rebuilt = String::new();
        let mut current_label: Option<String> = None;
        for r in rows {
            let text: String = r.try_get("text")?;
            let speaker: Option<String> = r.try_get("speaker")?;
            let Some(label) = speaker else { continue };
            if current_label.as_deref() != Some(label.as_str()) {
                if !rebuilt.is_empty() {
                    rebuilt.push_str("\n\n");
                }
                rebuilt.push_str(&format!("[Speaker {label}]: "));
                current_label = Some(label);
            } else {
                rebuilt.push(' ');
            }
            rebuilt.push_str(text.trim());
        }
        sqlx::query(
            "UPDATE recordings SET transcript = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(&rebuilt)
        .bind(recording_id.as_str())
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// Replace a recording's machine transcript words with a fresh set.
    ///
    /// The word-level twin of [`replace_segments`](Self::replace_segments):
    /// called by the pipeline after every transcribe/retranscribe, so the old
    /// rows are dropped first in the same transaction (a crash can't leave a
    /// half-replaced word timeline). An empty slice simply clears them — the
    /// normal state for a provider that emits no per-word timing.
    pub async fn replace_words(
        &self,
        recording_id: &RecordingId,
        words: &[TranscriptWord],
    ) -> Result<()> {
        self.replace_words_variant(recording_id, "raw", words).await
    }

    /// Replace one timing variant of a recording's words (`"raw"` machine-truth or
    /// `"cleaned"`, re-aligned to the post-cleanup text), leaving the other intact
    /// (TL-CONSISTENCY). The word-level twin of
    /// [`replace_segments_variant`](Self::replace_segments_variant).
    pub async fn replace_words_variant(
        &self,
        recording_id: &RecordingId,
        variant: &str,
        words: &[TranscriptWord],
    ) -> Result<()> {
        let table = words_table(variant);
        let mut tx = self.pool.begin().await?;
        sqlx::query(&format!("DELETE FROM {table} WHERE recording_id = ?"))
            .bind(recording_id.as_str())
            .execute(&mut *tx)
            .await?;
        for (idx, word) in words.iter().enumerate() {
            sqlx::query(&format!(
                "INSERT INTO {table} (recording_id, idx, start_ms, end_ms, text, speaker, confidence, leading_space) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            ))
            .bind(recording_id.as_str())
            .bind(idx as i64)
            .bind(word.start_ms)
            .bind(word.end_ms)
            .bind(&word.text)
            .bind(&word.speaker)
            .bind(word.confidence)
            .bind(word.leading_space)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// A recording's machine transcript words in timeline order. Empty when the
    /// recording predates word capture or its provider returned no per-word
    /// timing — callers must treat "no words" as a normal state, not an error.
    pub async fn words_for(&self, recording_id: &RecordingId) -> Result<Vec<TranscriptWord>> {
        self.words_for_variant(recording_id, "raw").await
    }

    /// A recording's words for one timing `variant` (`"raw"` or `"cleaned"`), in
    /// timeline order. Empty when that variant has no rows — a normal state.
    pub async fn words_for_variant(
        &self,
        recording_id: &RecordingId,
        variant: &str,
    ) -> Result<Vec<TranscriptWord>> {
        let table = words_table(variant);
        let rows = sqlx::query(&format!(
            "SELECT start_ms, end_ms, text, speaker, confidence, leading_space FROM {table} \
             WHERE recording_id = ? ORDER BY idx"
        ))
        .bind(recording_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(TranscriptWord {
                    start_ms: r.try_get("start_ms")?,
                    end_ms: r.try_get("end_ms")?,
                    text: r.try_get("text")?,
                    // Stored as INTEGER (0/1); powers the Synced view's spacing.
                    leading_space: r.try_get::<i64, _>("leading_space")? != 0,
                    speaker: r.try_get("speaker")?,
                    confidence: r.try_get("confidence")?,
                })
            })
            .collect()
    }
}

/// Decode one `(id, vector, meeting_id)` embedding row into a [`CachedVector`].
///
/// The vector is stored as little-endian f32 bytes; a blob whose length isn't a
/// multiple of 4 is kept as `vector: None` (and warned) so the ranking paths
/// skip it exactly as they did when decoding inline — the cache must not silently
/// resurrect a corrupt blob as a zero-length vector.
fn row_to_cached_vector(row: &sqlx::sqlite::SqliteRow) -> Result<CachedVector> {
    let id: String = row.try_get("id")?;
    let meeting_id: Option<String> = row.try_get("meeting_id")?;
    let bytes: Vec<u8> = row.try_get("vector")?;
    let vector = if bytes.len().is_multiple_of(4) {
        Some(
            bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect(),
        )
    } else {
        tracing::warn!(id = %id, len = bytes.len(), "skipping embedding: not 4-byte aligned");
        None
    };
    Ok(CachedVector {
        id,
        meeting_id,
        vector,
    })
}

fn row_to_recording(row: sqlx::sqlite::SqliteRow) -> Result<Recording> {
    let id: String = row.try_get("id")?;
    let started_at: String = row.try_get("started_at")?;
    let status: String = row.try_get("status")?;
    Ok(Recording {
        id: RecordingId::from_str_unchecked(&id),
        started_at: parse_dt(&started_at)?,
        duration_ms: row.try_get("duration_ms")?,
        audio_path: row.try_get("audio_path")?,
        transcript: row.try_get("transcript")?,
        model: row.try_get("model")?,
        status: parse_status(&status)?,
        error_kind: row.try_get("error_kind")?,
        error_message: row.try_get("error_message")?,
        hook_command: row.try_get("hook_command")?,
        hook_exit_code: row.try_get("hook_exit_code")?,
        hook_duration_ms: row.try_get("hook_duration_ms")?,
        transcribed_at: row
            .try_get::<Option<String>, _>("transcribed_at")?
            .map(|s| parse_dt(&s))
            .transpose()?,
        hook_ran_at: row
            .try_get::<Option<String>, _>("hook_ran_at")?
            .map(|s| parse_dt(&s))
            .transpose()?,
        notes: row.try_get("notes")?,
        meeting_id: row.try_get("meeting_id")?,
        meeting_name: row.try_get("meeting_name")?,
        track: row.try_get("track")?,
        in_place: row.try_get("in_place").unwrap_or(false),
        cleanup_model: row.try_get("cleanup_model").unwrap_or(None),
        diarized: row.try_get("diarized").unwrap_or(false),
        user_edited: row.try_get("user_edited").unwrap_or(false),
        favorite: row.try_get("favorite").unwrap_or(false),
        tag_suggestions: row
            .try_get::<Option<String>, _>("tag_suggestions")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        summary: row.try_get("summary").unwrap_or(None),
        summary_model: row.try_get("summary_model").unwrap_or(None),
        title: row.try_get("title").unwrap_or(None),
        title_is_auto: row.try_get("title_is_auto").unwrap_or(true),
        title_model: row.try_get("title_model").unwrap_or(None),
        tag_model: row.try_get("tag_model").unwrap_or(None),
        diarization_model: row.try_get("diarization_model").unwrap_or(None),
        tags: Vec::new(),
        // Populated separately (joined from `speaker_names`) by list/get/list_by_meeting.
        speaker_names: Vec::new(),
    })
}

fn parse_dt(s: &str) -> Result<DateTime<Local>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Local))
        .or_else(|_| {
            // SQLite's datetime('now') returns "YYYY-MM-DD HH:MM:SS" UTC.
            let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .map_err(|e| crate::error::Error::Internal(format!("bad datetime {s}: {e}")))?;
            Ok(chrono::TimeZone::from_utc_datetime(&chrono::Utc, &naive).with_timezone(&Local))
        })
}

fn parse_status(s: &str) -> Result<RecordingStatus> {
    Ok(match s {
        "recording" => RecordingStatus::Recording,
        "paused" => RecordingStatus::Paused,
        "queued" => RecordingStatus::Queued,
        "transcribing" => RecordingStatus::Transcribing,
        "cleaning_up" => RecordingStatus::CleaningUp,
        "summarizing" => RecordingStatus::Summarizing,
        "tagging" => RecordingStatus::Tagging,
        "hook_running" => RecordingStatus::HookRunning,
        "done" => RecordingStatus::Done,
        "transcribe_failed" => RecordingStatus::TranscribeFailed,
        "hook_failed" => RecordingStatus::HookFailed,
        "cleanup_failed" => RecordingStatus::CleanupFailed,
        "summarize_failed" => RecordingStatus::SummarizeFailed,
        "title_failed" => RecordingStatus::TitleFailed,
        "tag_failed" => RecordingStatus::TagFailed,
        "cancelled" => RecordingStatus::Cancelled,
        other => {
            return Err(crate::error::Error::Internal(format!(
                "unknown recording status: {other}"
            )))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_ignore_case_handles_counts_overlap_and_unicode() {
        // Basic multi-match, case-insensitive.
        let (n, s) = replace_ignore_case("The THE the", "the", "x");
        assert_eq!(n, 3);
        assert_eq!(s, "x x x");

        // No match → zero, original returned.
        let (n, s) = replace_ignore_case("hello", "zzz", "q");
        assert_eq!(n, 0);
        assert_eq!(s, "hello");

        // Replacement containing the needle does NOT recurse (we advance past the
        // inserted text), so "a" → "aa" over "aaa" yields exactly 3 replacements.
        let (n, s) = replace_ignore_case("aaa", "a", "aa");
        assert_eq!(n, 3);
        assert_eq!(s, "aaaaaa");

        // Unicode needle, mixed case, with surrounding multibyte text intact.
        let (n, s) = replace_ignore_case("Café CAFÉ déjà", "café", "tea");
        assert_eq!(n, 2);
        assert_eq!(s, "tea tea déjà");
    }

    #[test]
    fn parse_status_round_trips_all_variants_incl_paused() {
        // Regression: `parse_status` lacked a "paused" arm, so the moment a
        // recording was paused, `catalog.list()`/`get()` failed for the ENTIRE
        // result set (one bad row errored the whole query). Every status the DB
        // can hold must round-trip.
        for s in [
            "recording",
            "paused",
            "queued",
            "transcribing",
            "cleaning_up",
            "summarizing",
            "tagging",
            "hook_running",
            "done",
            "transcribe_failed",
            "hook_failed",
            "cleanup_failed",
            "summarize_failed",
            "title_failed",
            "tag_failed",
            "cancelled",
        ] {
            assert_eq!(
                parse_status(s).unwrap().as_str(),
                s,
                "status {s} did not round-trip through parse_status/as_str"
            );
        }
    }

    #[tokio::test]
    async fn update_error_persists_kind_and_message_and_a_retry_clears_them() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();

        // A failure writes both columns on the row itself, so the reason
        // survives a restart (not just the live event / quarantine JSON).
        db.update_error(&r.id, "whisper_error", "the model file is missing")
            .await
            .unwrap();
        let got = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(got.error_kind.as_deref(), Some("whisper_error"));
        assert_eq!(
            got.error_message.as_deref(),
            Some("the model file is missing")
        );

        // A later successful (re-)transcription clears the stale failure reason
        // so the recording no longer reads as failed.
        db.update_transcript(&r.id, "clean text", "raw text", "tiny")
            .await
            .unwrap();
        let got = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(got.error_kind, None, "a successful retry clears error_kind");
        assert_eq!(
            got.error_message, None,
            "a successful retry clears error_message"
        );
    }

    #[tokio::test]
    async fn ai_activity_round_trips_filters_and_orders_newest_first() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        // Migration applied: the table exists and starts empty.
        assert!(db.list_ai_activity(None, 50).await.unwrap().is_empty());

        // Two sessions for recording "a", one for "b".
        db.insert_ai_activity("a", "cleaning_up", "p1", "r1")
            .await
            .unwrap();
        db.insert_ai_activity("b", "summarizing", "p2", "r2")
            .await
            .unwrap();
        db.insert_ai_activity("a", "summarizing", "p3", "r3")
            .await
            .unwrap();

        // Global list is newest-first (id DESC == insert order DESC).
        let all = db.list_ai_activity(None, 50).await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].response, "r3", "most recent session first");
        assert_eq!(all[2].response, "r1", "oldest session last");

        // The per-recording filter returns only that recording's sessions.
        let a = db.list_ai_activity(Some("a"), 50).await.unwrap();
        assert_eq!(a.len(), 2);
        assert!(a.iter().all(|e| e.recording_id == "a"));
        assert_eq!(a[0].stage, "summarizing", "newest 'a' session first");
        assert_eq!(a[1].stage, "cleaning_up");

        // `limit` caps the result.
        assert_eq!(db.list_ai_activity(None, 1).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn ai_activity_caps_oversized_prompt_and_response() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

        // A normal-sized field round-trips verbatim — the popout still sees it all.
        db.insert_ai_activity("a", "cleaning_up", "short prompt", "short response")
            .await
            .unwrap();
        let small = &db.list_ai_activity(Some("a"), 1).await.unwrap()[0];
        assert_eq!(small.prompt, "short prompt");
        assert_eq!(small.response, "short response");

        // A field past the cap is truncated on a char boundary with a marker, so
        // the stored row can't grow without bound. Use multi-byte chars to prove
        // truncation never splits one (a byte-offset cut would panic or corrupt).
        let huge = "é".repeat(AI_ACTIVITY_FIELD_MAX_CHARS + 500);
        db.insert_ai_activity("b", "summarizing", &huge, &huge)
            .await
            .unwrap();
        let big = &db.list_ai_activity(Some("b"), 1).await.unwrap()[0];
        assert!(big.prompt.ends_with("… [truncated]"), "prompt not marked");
        assert!(
            big.response.ends_with("… [truncated]"),
            "response not marked"
        );
        // Kept chars = the cap; the marker is the only thing past it.
        let kept = big.prompt.chars().take(AI_ACTIVITY_FIELD_MAX_CHARS).count();
        assert_eq!(kept, AI_ACTIVITY_FIELD_MAX_CHARS);
        assert!(big.prompt.chars().count() < huge.chars().count());
    }

    #[tokio::test]
    async fn list_filters_kind_and_favorites_in_sql_before_pagination() {
        // Regression for the client-side Library type-filter: filtering kind /
        // favorites AFTER pagination meant a page could contain almost none of
        // the chosen kind, and favorites past the first page were unreachable.
        // Both predicates must be applied in the SQL, before LIMIT/OFFSET.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        // 30 single voice notes, the first 25 of them starred…
        let mut singles = Vec::new();
        for i in 0..30 {
            let r = embedded_recording(None);
            db.insert(&r).await.unwrap();
            if i < 25 {
                db.set_favorite(&r.id, true).await.unwrap();
            }
            singles.push(r.id);
        }
        // …plus 5 meeting tracks (never starred).
        for _ in 0..5 {
            db.insert(&embedded_recording(Some("meeting-1")))
                .await
                .unwrap();
        }

        // Kind filters match on meeting_id presence, across the whole set.
        let single_only = db
            .list(&ListFilter {
                kind: Some(crate::types::ListKind::Single),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(single_only.len(), 30);
        assert!(single_only.iter().all(|r| r.meeting_id.is_none()));

        let meeting_only = db
            .list(&ListFilter {
                kind: Some(crate::types::ListKind::Meeting),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(meeting_only.len(), 5);
        assert!(meeting_only.iter().all(|r| r.meeting_id.is_some()));

        // THE crux: page 3 of the favorites view (limit 10, offset 20) must
        // hold the remaining 5 starred recordings — with post-pagination
        // filtering this page was empty or full of unstarred rows.
        let fav_page3 = db
            .list(&ListFilter {
                favorite: Some(true),
                limit: Some(10),
                offset: Some(20),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            fav_page3.len(),
            5,
            "favorites beyond page 1 must be reachable (25 favorites → 5 on page 3)"
        );
        assert!(fav_page3.iter().all(|r| r.favorite));

        // Some(false) is the complement: only unstarred rows.
        let unstarred = db
            .list(&ListFilter {
                favorite: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            unstarred.len(),
            10,
            "5 unstarred singles + 5 meeting tracks"
        );
        assert!(unstarred.iter().all(|r| !r.favorite));

        // Kind + favorites compose.
        let fav_singles = db
            .list(&ListFilter {
                kind: Some(crate::types::ListKind::Single),
                favorite: Some(true),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(fav_singles.len(), 25);
    }

    #[test]
    fn test_sanitize_fts5_query() {
        assert_eq!(sanitize_fts5_query("hello"), "hello*");
        assert_eq!(sanitize_fts5_query("hello world"), "hello* AND world*");
        assert_eq!(sanitize_fts5_query("O'Connor"), "O* AND Connor*");
        assert_eq!(
            sanitize_fts5_query("some-bad*characters"),
            "some* AND bad* AND characters*"
        );
        assert_eq!(sanitize_fts5_query("\"quotes\""), "quotes*");
        assert_eq!(sanitize_fts5_query("   spaces   "), "spaces*");
        assert_eq!(sanitize_fts5_query(""), "");
    }

    /// A minimal `Done` recording for embedding/search tests. `semantic_search`
    /// JOINs embeddings to recordings, so the row must exist before embedding.
    fn embedded_recording(meeting_id: Option<&str>) -> Recording {
        Recording {
            id: RecordingId::new(),
            started_at: Local::now(),
            duration_ms: 1000,
            audio_path: "x.wav".into(),
            transcript: Some("t".into()),
            model: Some("tiny".into()),
            status: RecordingStatus::Done,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
            notes: None,
            meeting_id: meeting_id.map(|s| s.to_string()),
            meeting_name: None,
            track: None,
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            tags: vec![],
            speaker_names: vec![],
        }
    }

    #[tokio::test]
    async fn semantic_search_ranks_by_cosine_and_respects_limit() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        let b = embedded_recording(None);
        let c = embedded_recording(None);
        for r in [&a, &b, &c] {
            db.insert(r).await.unwrap();
        }
        // Orthonormal vectors: query [1,0,0] is identical to `a`, orthogonal to b/c.
        db.upsert_embedding(&a.id, &[1.0, 0.0, 0.0]).await.unwrap();
        db.upsert_embedding(&b.id, &[0.0, 1.0, 0.0]).await.unwrap();
        db.upsert_embedding(&c.id, &[0.0, 0.0, 1.0]).await.unwrap();

        // min_score -1.0 keeps everything so we can assert ordering.
        let results = db
            .semantic_search(&[1.0, 0.0, 0.0], 10, -1.0)
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0.as_str(), a.id.as_str(), "best match first");
        assert!(
            (results[0].1 - 1.0).abs() < 1e-6,
            "identical vector scores ~1.0"
        );
        assert!(
            results[0].1 >= results[1].1 && results[1].1 >= results[2].1,
            "results must be sorted high→low"
        );

        // `limit` caps the result count.
        let top1 = db.semantic_search(&[1.0, 0.0, 0.0], 1, -1.0).await.unwrap();
        assert_eq!(top1.len(), 1);
        assert_eq!(top1[0].0.as_str(), a.id.as_str());
    }

    #[tokio::test]
    async fn semantic_search_min_score_filters_low_matches() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        db.insert(&a).await.unwrap();
        db.upsert_embedding(&a.id, &[1.0, 0.0, 0.0]).await.unwrap();
        // Orthogonal query → cosine 0.0, under a 0.5 floor → dropped.
        let results = db.semantic_search(&[0.0, 1.0, 0.0], 10, 0.5).await.unwrap();
        assert!(
            results.is_empty(),
            "below-floor matches must be filtered out"
        );
    }

    #[tokio::test]
    async fn semantic_search_skips_dimension_mismatch_without_panicking() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let good = embedded_recording(None);
        let bad = embedded_recording(None);
        db.insert(&good).await.unwrap();
        db.insert(&bad).await.unwrap();
        db.upsert_embedding(&good.id, &[1.0, 0.0, 0.0])
            .await
            .unwrap();
        // Wrong dimension (2 vs the query's 3) — must be skipped, not scored on a
        // truncated prefix and not panic.
        db.upsert_embedding(&bad.id, &[1.0, 0.0]).await.unwrap();

        let results = db
            .semantic_search(&[1.0, 0.0, 0.0], 10, -1.0)
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "the mismatched-dim embedding is skipped");
        assert_eq!(results[0].0.as_str(), good.id.as_str());
    }

    #[tokio::test]
    async fn semantic_search_collapses_meeting_tracks() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let mic = embedded_recording(Some("meeting-1"));
        let sys = embedded_recording(Some("meeting-1"));
        let solo = embedded_recording(None);
        for r in [&mic, &sys, &solo] {
            db.insert(r).await.unwrap();
        }
        // Both tracks of meeting-1 are highly similar to the query; solo isn't.
        db.upsert_embedding(&mic.id, &[1.0, 0.0, 0.0])
            .await
            .unwrap();
        db.upsert_embedding(&sys.id, &[0.99, 0.01, 0.0])
            .await
            .unwrap();
        db.upsert_embedding(&solo.id, &[0.0, 1.0, 0.0])
            .await
            .unwrap();

        let results = db
            .semantic_search(&[1.0, 0.0, 0.0], 10, -1.0)
            .await
            .unwrap();
        // The meeting's two tracks collapse to one entry (best-scoring track),
        // alongside the standalone recording.
        assert_eq!(results.len(), 2);
        let meeting_hits = results
            .iter()
            .filter(|(id, _)| id.as_str() == mic.id.as_str() || id.as_str() == sys.id.as_str())
            .count();
        assert_eq!(
            meeting_hits, 1,
            "meeting tracks must collapse to one result"
        );
    }

    #[tokio::test]
    async fn upsert_chunk_embeddings_replaces_prior_chunks() {
        // Re-embedding (a re-transcription or a manual edit) must REPLACE a
        // recording's chunk vectors, never leave stale ones from the old text
        // behind — otherwise an edited note keeps matching phrases it no longer
        // contains. We store three chunks, then re-embed with two and assert the
        // third is gone.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        db.insert(&a).await.unwrap();

        db.upsert_chunk_embeddings(
            &a.id,
            &[
                vec![1.0, 0.0, 0.0],
                vec![0.0, 1.0, 0.0],
                vec![0.0, 0.0, 1.0],
            ],
        )
        .await
        .unwrap();

        // A query identical to the second chunk finds the recording.
        let r = db.vector_ranking(&[0.0, 1.0, 0.0]).await.unwrap();
        assert_eq!(r.len(), 1);
        assert!((r[0].2 - 1.0).abs() < 1e-6, "best chunk is the exact match");

        // Re-embed with only two chunks; the third (z-axis) must be dropped.
        db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]])
            .await
            .unwrap();
        // The old z-axis chunk is gone: a z-axis query now only matches by the
        // shared positive baseline (here, exactly 0 against the two remaining
        // orthogonal chunks), not 1.0.
        let r2 = db.vector_ranking(&[0.0, 0.0, 1.0]).await.unwrap();
        assert!(
            r2.is_empty() || r2[0].2 < 0.5,
            "stale chunk must not survive a re-embed (got {r2:?})"
        );

        // Empty re-embed clears all chunks.
        db.upsert_chunk_embeddings(&a.id, &[]).await.unwrap();
        let none = db.list_recordings_without_chunk_embeddings().await.unwrap();
        assert!(
            none.iter().any(|rec| rec.id.as_str() == a.id.as_str()),
            "after clearing, the recording reappears as needing chunks"
        );
    }

    #[tokio::test]
    async fn vector_ranking_scores_by_best_chunk_not_average() {
        // The core paraphrase fix: a recording is ranked by its BEST-matching
        // chunk (max-sim), not by an averaged whole-note vector. Recording `a`
        // has many unrelated chunks plus ONE chunk that nails the query; it must
        // still rank top, because that one chunk competes on its own tight vector
        // instead of being diluted by the rest of the note.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        let b = embedded_recording(None);
        db.insert(&a).await.unwrap();
        db.insert(&b).await.unwrap();

        // `a`: one chunk exactly on the query axis, several pulling other ways.
        db.upsert_chunk_embeddings(
            &a.id,
            &[
                vec![0.0, 1.0, 0.0],
                vec![0.0, 0.0, 1.0],
                vec![1.0, 0.0, 0.0], // the matching chunk
                vec![0.0, 1.0, 0.0],
            ],
        )
        .await
        .unwrap();
        // `b`: a single chunk only loosely aligned with the query.
        db.upsert_chunk_embeddings(&b.id, &[vec![0.6, 0.8, 0.0]])
            .await
            .unwrap();

        let ranking = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
        assert_eq!(ranking.len(), 2);
        assert_eq!(
            ranking[0].1.as_str(),
            a.id.as_str(),
            "the recording with the best single chunk wins (max-sim, not mean)"
        );
        assert!(
            (ranking[0].2 - 1.0).abs() < 1e-6,
            "best-chunk cosine is the exact-match chunk's score, not an average"
        );
    }

    #[tokio::test]
    async fn vector_ranking_falls_back_to_legacy_whole_recording_vector() {
        // During the backfill window a recording may still have only a legacy
        // whole-recording vector and no chunks. It must remain searchable via the
        // `embeddings` table fallback, and once chunks exist they SUPERSEDE the
        // legacy vector (no double-counting / no stale legacy score winning).
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let legacy_only = embedded_recording(None);
        let chunked = embedded_recording(None);
        db.insert(&legacy_only).await.unwrap();
        db.insert(&chunked).await.unwrap();

        // legacy_only: only the old whole-recording vector, loosely on-axis.
        db.upsert_embedding(&legacy_only.id, &[0.8, 0.6, 0.0])
            .await
            .unwrap();
        // chunked: a stale legacy vector AND a fresh, better chunk vector. The
        // chunk must win; the legacy row must be ignored for this recording.
        db.upsert_embedding(&chunked.id, &[0.0, 0.0, 1.0])
            .await
            .unwrap();
        db.upsert_chunk_embeddings(&chunked.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();

        let ranking = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
        assert_eq!(ranking.len(), 2, "both recordings are searchable");
        // chunked's fresh chunk (cosine 1.0) beats legacy_only's 0.8.
        assert_eq!(ranking[0].1.as_str(), chunked.id.as_str());
        assert!((ranking[0].2 - 1.0).abs() < 1e-6);
        // And the chunked recording is scored from its chunk, not its stale
        // legacy vector (which was orthogonal → would have scored 0.0).
        let legacy_score = ranking
            .iter()
            .find(|(_key, id, _score)| id.as_str() == legacy_only.id.as_str())
            .unwrap()
            .2;
        assert!(
            (legacy_score - 0.8).abs() < 1e-6,
            "legacy-only recording scored from its whole-recording vector"
        );
    }

    #[tokio::test]
    async fn embedding_cache_warms_and_returns_identical_results() {
        // (a) The cache must be transparent: a query warms the snapshot, and a
        // repeated query against the warm cache returns byte-identical rankings.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        let b = embedded_recording(None);
        let c = embedded_recording(None);
        for r in [&a, &b, &c] {
            db.insert(r).await.unwrap();
        }
        db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();
        db.upsert_chunk_embeddings(&b.id, &[vec![0.0, 1.0, 0.0]])
            .await
            .unwrap();
        db.upsert_chunk_embeddings(&c.id, &[vec![0.0, 0.0, 1.0]])
            .await
            .unwrap();

        // Writes leave the cache cold; the first query warms it.
        assert_eq!(db.cached_vector_count(), None, "cold after writes");
        let first = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
        assert_eq!(
            db.cached_vector_count(),
            Some(3),
            "first query warms the snapshot with all three chunk vectors"
        );

        // A second query reads from the warm cache and must produce the same
        // per-recording cosine scores. (The order *between equal scores* is not
        // a guarantee of either path — both build from a HashMap and sort
        // unstably — so compare on id→score, the contract that actually matters.)
        let second = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
        let as_scores = |r: &[(String, RecordingId, f32)]| {
            r.iter()
                .map(|(_k, id, s)| (id.as_str().to_string(), *s))
                .collect::<std::collections::HashMap<_, _>>()
        };
        assert_eq!(
            as_scores(&first),
            as_scores(&second),
            "a cached query returns the same per-recording scores as the cold one"
        );
        assert_eq!(first[0].1.as_str(), a.id.as_str(), "best match still first");
        assert!(
            (first[0].2 - 1.0).abs() < 1e-6,
            "the on-axis recording scores ~1.0 from the warm cache"
        );

        // hybrid_search shares the same cache; its top hit must be stable.
        let h1 = db
            .hybrid_search("x", &[1.0, 0.0, 0.0], 10, -1.0, None)
            .await
            .unwrap();
        let h2 = db
            .hybrid_search("x", &[1.0, 0.0, 0.0], 10, -1.0, None)
            .await
            .unwrap();
        let scores1: std::collections::HashMap<_, _> = h1
            .iter()
            .map(|(id, s)| (id.as_str().to_string(), *s))
            .collect();
        let scores2: std::collections::HashMap<_, _> = h2
            .iter()
            .map(|(id, s)| (id.as_str().to_string(), *s))
            .collect();
        assert_eq!(
            scores1, scores2,
            "hybrid_search yields the same per-recording relevance over the warm cache"
        );
    }

    #[tokio::test]
    async fn retention_hard_delete_invalidates_the_embedding_cache() {
        // A retention sweep that hard-deletes recordings cascade-drops their
        // embeddings; the warm cache MUST be invalidated or the deleted vectors
        // keep surfacing as ghost hits in search.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        db.insert(&a).await.unwrap();
        db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();
        db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap(); // warm the cache
        assert!(db.cached_vector_count().is_some(), "warm after the query");

        // Hard delete every terminal recording (max_count = 0, delete_audio off).
        let cfg = crate::config::RetentionConfig {
            max_count: Some(0),
            ..Default::default()
        };
        db.apply_retention(&cfg).await.unwrap();
        assert_eq!(
            db.cached_vector_count(),
            None,
            "a retention hard-delete must invalidate the embedding cache"
        );
    }

    #[tokio::test]
    async fn audio_only_retention_does_not_drop_the_cache() {
        // delete_audio mode only blanks audio_path (keeps row + embeddings), so
        // it must NOT needlessly drop a warm cache.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        db.insert(&a).await.unwrap();
        db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();
        db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
        assert!(db.cached_vector_count().is_some());
        let cfg = crate::config::RetentionConfig {
            max_count: Some(0),
            delete_audio: true,
            ..Default::default()
        };
        db.apply_retention(&cfg).await.unwrap();
        assert!(
            db.cached_vector_count().is_some(),
            "audio-only retention keeps embeddings, so the cache should stay warm"
        );
    }

    #[tokio::test]
    async fn reembed_invalidates_cache_and_changes_ranking() {
        // (b) The correctness invariant: a re-embed must invalidate the cache so
        // the changed vector takes effect. A stale cached vector returning the
        // old ranking would be the worst possible bug.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        let b = embedded_recording(None);
        db.insert(&a).await.unwrap();
        db.insert(&b).await.unwrap();

        // Initially `a` is on the query axis and wins; `b` is orthogonal.
        db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();
        db.upsert_chunk_embeddings(&b.id, &[vec![0.0, 1.0, 0.0]])
            .await
            .unwrap();

        let before = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
        assert_eq!(
            before[0].1.as_str(),
            a.id.as_str(),
            "a wins before re-embed"
        );
        assert!(db.cached_vector_count().is_some(), "warm after the query");

        // Re-embed `b` so it now nails the query and `a` becomes orthogonal —
        // this is the re-transcribe / ReembedAll write path.
        db.upsert_chunk_embeddings(&b.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();
        db.upsert_chunk_embeddings(&a.id, &[vec![0.0, 1.0, 0.0]])
            .await
            .unwrap();
        assert_eq!(
            db.cached_vector_count(),
            None,
            "the re-embed invalidated the snapshot"
        );

        let after = db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
        assert_eq!(
            after[0].1.as_str(),
            b.id.as_str(),
            "the changed vector flips the ranking — no stale cache"
        );

        // clear_all_embeddings (ReembedAll) must also invalidate.
        db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap(); // re-warm
        assert!(db.cached_vector_count().is_some());
        db.clear_all_embeddings().await.unwrap();
        assert_eq!(
            db.cached_vector_count(),
            None,
            "clear_all_embeddings invalidates the snapshot"
        );
        // And the legacy whole-recording path (semantic_search) invalidates too.
        db.upsert_embedding(&a.id, &[1.0, 0.0, 0.0]).await.unwrap();
        db.semantic_search(&[1.0, 0.0, 0.0], 10, -1.0)
            .await
            .unwrap();
        assert!(
            db.cached_vector_count().is_some(),
            "semantic_search warms it"
        );
        db.upsert_embedding(&a.id, &[0.0, 1.0, 0.0]).await.unwrap();
        assert_eq!(
            db.cached_vector_count(),
            None,
            "upsert_embedding invalidates the snapshot"
        );
        // A delete cascades embeddings away, so it must invalidate as well.
        db.semantic_search(&[1.0, 0.0, 0.0], 10, -1.0)
            .await
            .unwrap();
        assert!(db.cached_vector_count().is_some());
        db.delete(&a.id).await.unwrap();
        assert_eq!(
            db.cached_vector_count(),
            None,
            "delete invalidates the snapshot"
        );
    }

    #[tokio::test]
    async fn rebuild_does_not_clobber_an_invalidation_that_raced_it() {
        // The lost-invalidation TOCTOU: `embedding_corpus` snapshots the
        // generation BEFORE its SQL reads and only caches when the generation is
        // unchanged at store time. If `invalidate_embedding_cache` (a racing
        // embedding write) lands between the snapshot and the store, the store
        // MUST leave the slot cold so the writer's fresh data wins — otherwise a
        // pre-write snapshot would be cached and search would go stale.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        db.insert(&a).await.unwrap();
        db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();

        // Simulate the race exactly: take the gen snapshot a rebuild would take,
        // then let an invalidation land before the store.
        let gen_at_miss = db.embedding_cache_gen.load(Ordering::Acquire);
        let raced_corpus = Arc::new(EmbeddingCorpus {
            chunks: vec![CachedVector {
                id: a.id.as_str().to_string(),
                meeting_id: None,
                vector: Some(vec![1.0, 0.0, 0.0]),
            }],
            legacy: vec![],
        });
        db.invalidate_embedding_cache(); // the racing write bumps the generation
        db.store_corpus_if_current(raced_corpus, gen_at_miss);
        assert_eq!(
            db.cached_vector_count(),
            None,
            "a snapshot taken before a racing invalidation must NOT be cached"
        );

        // Control: with no racing invalidation, the same store DOES cache (so the
        // guard isn't just refusing to ever cache).
        let gen_now = db.embedding_cache_gen.load(Ordering::Acquire);
        let fresh_corpus = Arc::new(EmbeddingCorpus {
            chunks: vec![CachedVector {
                id: a.id.as_str().to_string(),
                meeting_id: None,
                vector: Some(vec![1.0, 0.0, 0.0]),
            }],
            legacy: vec![],
        });
        db.store_corpus_if_current(fresh_corpus, gen_now);
        assert_eq!(
            db.cached_vector_count(),
            Some(1),
            "an uncontested store caches the snapshot"
        );
    }

    #[tokio::test]
    async fn warm_cache_hit_shares_the_same_corpus_arc() {
        // Fix 2: a warm hit returns the SAME Arc (O(1) clone), not a deep copy of
        // every vector. Two reads with no intervening write must hand back Arcs
        // that point at one allocation.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        db.insert(&a).await.unwrap();
        db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();
        let first = db.embedding_corpus().await.unwrap(); // miss → caches
        let second = db.embedding_corpus().await.unwrap(); // warm hit
        assert!(
            Arc::ptr_eq(&first, &second),
            "a warm hit must clone the Arc, not deep-copy the corpus"
        );
    }

    #[tokio::test]
    async fn embedding_cache_is_bounded_by_the_vector_cap() {
        // (c) The cache must not grow without bound. The loader stores a corpus
        // only when `chunks + legacy <= MAX_CACHED_VECTORS`; an over-cap corpus
        // takes the else branch and stays uncached (queries fall back to SQLite),
        // so memory is bounded no matter how large the library grows. We exercise
        // that decision through `cap_allows_caching`, then confirm a real small
        // corpus is in fact cached and never exceeds the cap — without inserting
        // hundreds of thousands of rows.
        assert!(
            Catalog::cap_allows_caching(MAX_CACHED_VECTORS),
            "a corpus exactly at the cap is still cached"
        );
        assert!(
            !Catalog::cap_allows_caching(MAX_CACHED_VECTORS + 1),
            "a corpus one over the cap is NOT cached, so the cache is bounded"
        );

        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        db.insert(&a).await.unwrap();
        // A small corpus (1 vector) is comfortably under the cap and IS cached.
        db.upsert_chunk_embeddings(&a.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();
        db.vector_ranking(&[1.0, 0.0, 0.0]).await.unwrap();
        let count = db.cached_vector_count().expect("a small corpus is cached");
        assert!(
            count <= MAX_CACHED_VECTORS,
            "a cached corpus never exceeds the cap"
        );
    }

    #[tokio::test]
    async fn more_like_this_excludes_source_and_ranks_by_similarity() {
        // The recall flow: open a recording → find its semantic neighbours from
        // the vectors already in the catalog. The source itself must never be a
        // result, neighbours come back best-first with calibrated scores, and
        // near-orthogonal noise is dropped by the relevance floor.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let source = embedded_recording(None);
        let close = embedded_recording(None);
        let loose = embedded_recording(None);
        let unrelated = embedded_recording(None);
        for r in [&source, &close, &loose, &unrelated] {
            db.insert(r).await.unwrap();
        }

        // Source has two chunks; its (renormalized) mean is ~[0.707, 0.707, 0].
        db.upsert_chunk_embeddings(&source.id, &[vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]])
            .await
            .unwrap();
        // close: cosine vs the centroid ≈ 0.707 (calibrates to 1.0, the ceiling).
        db.upsert_chunk_embeddings(&close.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();
        // loose: cosine ≈ 0.42 (calibrates to ~0.5 — clearly mid-strength).
        db.upsert_chunk_embeddings(&loose.id, &[vec![0.6, 0.0, 0.8]])
            .await
            .unwrap();
        // unrelated: orthogonal to the centroid → calibrated 0 → floored out.
        db.upsert_chunk_embeddings(&unrelated.id, &[vec![0.0, 0.0, 1.0]])
            .await
            .unwrap();

        let results = db.more_like_this(&source.id, 10, 0.12).await.unwrap();
        let ids: Vec<&str> = results.iter().map(|(id, _)| id.as_str()).collect();
        assert!(
            !ids.contains(&source.id.as_str()),
            "the source recording must never be in its own results"
        );
        assert_eq!(
            ids,
            vec![close.id.as_str(), loose.id.as_str()],
            "neighbours best-first; orthogonal noise floored out"
        );
        assert!(
            results[0].1 > results[1].1,
            "scores must be calibrated and descending, got {results:?}"
        );

        // `limit` caps the result count at the top of the ranking.
        let top1 = db.more_like_this(&source.id, 1, 0.12).await.unwrap();
        assert_eq!(top1.len(), 1);
        assert_eq!(top1[0].0.as_str(), close.id.as_str());
    }

    #[tokio::test]
    async fn more_like_this_excludes_the_sources_own_meeting_sibling() {
        // A meeting's two tracks have near-identical transcripts, so the
        // sibling track would always trivially rank #1 — useless as a
        // recommendation. Exclusion is by the meeting dedupe key, so the
        // sibling is dropped along with the source while other recordings
        // still surface.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let mic = embedded_recording(Some("meeting-1"));
        let sys = embedded_recording(Some("meeting-1"));
        let other = embedded_recording(None);
        for r in [&mic, &sys, &other] {
            db.insert(r).await.unwrap();
        }
        db.upsert_chunk_embeddings(&mic.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();
        db.upsert_chunk_embeddings(&sys.id, &[vec![0.99, 0.01, 0.0]])
            .await
            .unwrap();
        db.upsert_chunk_embeddings(&other.id, &[vec![0.9, 0.1, 0.0]])
            .await
            .unwrap();

        let results = db.more_like_this(&mic.id, 10, 0.12).await.unwrap();
        let ids: Vec<&str> = results.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            vec![other.id.as_str()],
            "the source's own meeting sibling must be excluded, got {ids:?}"
        );
    }

    #[tokio::test]
    async fn more_like_this_falls_back_to_the_legacy_whole_recording_vector() {
        // A source from before per-chunk embedding (backfill pending) still
        // works: its legacy whole-recording vector is the query.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let legacy_source = embedded_recording(None);
        let neighbour = embedded_recording(None);
        db.insert(&legacy_source).await.unwrap();
        db.insert(&neighbour).await.unwrap();
        db.upsert_embedding(&legacy_source.id, &[1.0, 0.0, 0.0])
            .await
            .unwrap();
        db.upsert_chunk_embeddings(&neighbour.id, &[vec![0.95, 0.05, 0.0]])
            .await
            .unwrap();

        let results = db
            .more_like_this(&legacy_source.id, 10, 0.12)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.as_str(), neighbour.id.as_str());
    }

    #[tokio::test]
    async fn more_like_this_errors_clearly_when_source_missing_or_not_indexed() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

        // Unknown id → NotFound (not the "not indexed" message).
        let ghost = RecordingId::new();
        let err = db.more_like_this(&ghost, 10, 0.12).await.unwrap_err();
        assert!(
            matches!(err, crate::error::Error::NotFound { .. }),
            "missing recording must be NotFound, got {err:?}"
        );

        // Existing but never embedded → a clear "not indexed yet" error the UI
        // can show verbatim.
        let bare = embedded_recording(None);
        db.insert(&bare).await.unwrap();
        let err = db.more_like_this(&bare.id, 10, 0.12).await.unwrap_err();
        assert!(
            matches!(&err, crate::error::Error::Internal(msg) if msg.contains("isn't indexed")),
            "unembedded recording must report it isn't indexed, got {err:?}"
        );
    }

    #[tokio::test]
    async fn hybrid_search_recalls_a_paraphrase_where_keyword_match_misses() {
        // THE headline requirement: "utter the likeness of something I spoke
        // about and get the proper search results."
        //
        // We simulate the embedding space directly (the ONNX model isn't bundled
        // in tests). The query and the target recording's transcript share NO
        // word, so FTS5 (lexical) returns nothing for them — a naive keyword
        // search misses entirely. But their *vectors* are nearly identical
        // (high cosine), modelling a paraphrase. Hybrid search must still surface
        // the right recording, ranked first, with an honest relevance score.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

        // The recording the user is trying to recall. Its transcript talks about
        // moving the schema over — the QUERY below ("database migration") shares
        // none of these words, so lexical search cannot find it.
        let mut target = embedded_recording(None);
        target.transcript = Some("we should shift the records across to the new store".into());
        // A distractor whose words overlap the query's domain words a bit but
        // whose meaning (and vector) is unrelated.
        let mut distractor = embedded_recording(None);
        distractor.transcript = Some("lunch plans for friday afternoon".into());
        db.insert(&target).await.unwrap();
        db.insert(&distractor).await.unwrap();

        // Query vector ("the bit about the database migration"). The target's
        // matching chunk vector is nearly identical (paraphrase); the distractor
        // points elsewhere.
        let query_vec = [1.0_f32, 0.0, 0.0];
        db.upsert_chunk_embeddings(&target.id, &[vec![0.98, 0.20, 0.0]])
            .await
            .unwrap();
        db.upsert_chunk_embeddings(&distractor.id, &[vec![0.0, 0.0, 1.0]])
            .await
            .unwrap();

        // Sanity: a pure keyword search for the query terms finds NOTHING — the
        // words don't appear in either transcript. This is the gap vectors close.
        let lexical = db.lexical_ranking("database migration").await.unwrap();
        assert!(
            lexical.is_empty(),
            "precondition: naive keyword search must miss the paraphrase"
        );

        // Hybrid search, same min_relevance the daemon uses (0.12). Despite the
        // lexical miss, the semantic signal surfaces the target, ranked first.
        let results = db
            .hybrid_search("database migration", &query_vec, 10, 0.12, None)
            .await
            .unwrap();
        assert!(
            !results.is_empty(),
            "paraphrase must be recalled by meaning"
        );
        assert_eq!(
            results[0].0.as_str(),
            target.id.as_str(),
            "the paraphrased recording must rank first"
        );
        // The displayed relevance is the calibrated best-chunk cosine — a strong
        // paraphrase (cosine ~0.98) should read as a strong match, not single
        // digits.
        assert!(
            results[0].1 > 0.5,
            "a strong paraphrase should read as a strong relevance, got {}",
            results[0].1
        );
    }

    #[tokio::test]
    async fn hybrid_search_keeps_exact_term_hit_despite_weak_cosine() {
        // The complement to paraphrase recall: when the user remembers one
        // distinctive word, an exact lexical hit must surface even if its vector
        // barely aligns with the query — never filtered out by the relevance
        // floor. This is the "union of strengths" guarantee.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let mut named = embedded_recording(None);
        named.transcript = Some("the Kubernetes rollout notes are attached".into());
        db.insert(&named).await.unwrap();
        // Its vector is essentially orthogonal to the query (weak cosine), so a
        // semantic-only path with a 0.12 floor would drop it.
        db.upsert_chunk_embeddings(&named.id, &[vec![0.0, 1.0, 0.0]])
            .await
            .unwrap();

        // The user types the exact distinctive term; the query vector is the
        // unrelated x-axis.
        let results = db
            .hybrid_search("Kubernetes", &[1.0, 0.0, 0.0], 10, 0.12, None)
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "the exact-term hit must survive");
        assert_eq!(results[0].0.as_str(), named.id.as_str());
        assert!(
            results[0].1 > 0.0,
            "a lexical-only hit gets an honest non-zero relevance floor, not 0%"
        );
    }

    #[tokio::test]
    async fn hybrid_search_collapses_a_meeting_across_both_retrievers() {
        // Regression for the cross-retriever dedupe: a meeting's two tracks share
        // a meeting_id. If the vector retriever's best track differs from the
        // lexical retriever's best track, fusing on raw recording id would surface
        // the SAME meeting twice. Fusing on the meeting-stable dedupe key must
        // collapse it to one row.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let mic = embedded_recording(Some("meeting-x"));
        let mut sys = embedded_recording(Some("meeting-x"));
        // Put the distinctive lexical term on the SYSTEM track only, and the
        // strong semantic vector on the MIC track only — so each retriever prefers
        // a different track of the same meeting.
        sys.transcript = Some("the quarterly Kubernetes review".into());
        db.insert(&mic).await.unwrap();
        db.insert(&sys).await.unwrap();

        // Mic track: chunk vector strongly on the query axis (semantic winner).
        db.upsert_chunk_embeddings(&mic.id, &[vec![1.0, 0.0, 0.0]])
            .await
            .unwrap();
        // System track: vector points elsewhere, but it carries the exact term.
        db.upsert_chunk_embeddings(&sys.id, &[vec![0.0, 1.0, 0.0]])
            .await
            .unwrap();

        let results = db
            .hybrid_search("Kubernetes", &[1.0, 0.0, 0.0], 10, 0.12, None)
            .await
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "the meeting's two tracks must collapse to a single result, got {results:?}"
        );
        // The surviving row is one of the meeting's tracks.
        assert!(
            results[0].0.as_str() == mic.id.as_str() || results[0].0.as_str() == sys.id.as_str()
        );
    }

    #[tokio::test]
    async fn test_insert_and_get() {
        let db = Catalog::open(Path::new("sqlite::memory:"))
            .await
            .expect("open db");
        let r = Recording {
            id: RecordingId::new(),
            started_at: Local::now(),
            duration_ms: 5000,
            audio_path: "foo.wav".into(),
            transcript: Some("hello world".into()),
            model: Some("tiny".into()),
            status: RecordingStatus::Done,
            error_kind: None,
            error_message: None,
            hook_command: Some("to-stdout.ps1".into()),
            hook_exit_code: Some(0),
            hook_duration_ms: Some(100),
            transcribed_at: Some(Local::now()),
            hook_ran_at: Some(Local::now()),
            notes: None,
            meeting_id: None,
            meeting_name: None,
            track: None,
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            tags: vec![],
            speaker_names: vec![],
        };
        db.insert(&r).await.expect("insert");

        let fetched = db
            .get(&r.id)
            .await
            .expect("get recording")
            .expect("is some");
        assert_eq!(fetched.id.as_str(), r.id.as_str());
        assert_eq!(fetched.audio_path, r.audio_path);
        assert_eq!(fetched.transcript.as_deref(), Some("hello world"));
        assert_eq!(fetched.status, RecordingStatus::Done);

        // Test list
        let filter = ListFilter {
            limit: Some(10),
            ..Default::default()
        };
        let list = db.list(&filter).await.expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id.as_str(), r.id.as_str());
    }

    #[tokio::test]
    async fn original_transcript_preserved_across_user_edit() {
        let db = Catalog::open(Path::new("sqlite::memory:"))
            .await
            .expect("open db");
        let r = Recording {
            id: RecordingId::new(),
            started_at: Local::now(),
            duration_ms: 1000,
            audio_path: "x.wav".into(),
            transcript: None,
            model: None,
            status: RecordingStatus::Transcribing,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
            notes: None,
            meeting_id: None,
            meeting_name: None,
            track: None,
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            tags: vec![],
            speaker_names: vec![],
        };
        db.insert(&r).await.expect("insert");

        // Machine transcription stores transcript + original.
        db.update_transcript(&r.id, "machine output", "machine output", "ggml-base")
            .await
            .expect("machine transcript");
        assert_eq!(
            db.get_original_transcript(&r.id).await.unwrap().as_deref(),
            Some("machine output")
        );

        // A user edit changes the transcript but preserves the original.
        db.update_user_transcript(&r.id, "edited by the user")
            .await
            .expect("user edit");
        let got = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(got.transcript.as_deref(), Some("edited by the user"));
        // The transcription model is preserved — a hand edit is surfaced by the
        // user_edited flag / "Edited" column, not by overwriting the model field.
        assert_eq!(got.model.as_deref(), Some("ggml-base"));
        assert!(
            got.user_edited,
            "a manual edit must set the user_edited flag"
        );
        assert_eq!(
            db.get_original_transcript(&r.id).await.unwrap().as_deref(),
            Some("machine output")
        );
    }

    #[tokio::test]
    async fn notes_round_trip_and_survive_transcription() {
        let db = Catalog::open(Path::new("sqlite::memory:"))
            .await
            .expect("open db");
        let r = Recording {
            id: RecordingId::new(),
            started_at: Local::now(),
            duration_ms: 1000,
            audio_path: "x.wav".into(),
            transcript: None,
            model: None,
            status: RecordingStatus::Transcribing,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
            notes: None,
            meeting_id: None,
            meeting_name: None,
            track: None,
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            tags: vec![],
            speaker_names: vec![],
        };
        db.insert(&r).await.expect("insert");

        // Fresh insert: notes default to NULL.
        assert_eq!(db.get(&r.id).await.unwrap().unwrap().notes, None);

        // Notes round-trip through update_notes + get.
        db.update_notes(&r.id, "remember to follow up")
            .await
            .expect("update notes");
        assert_eq!(
            db.get(&r.id).await.unwrap().unwrap().notes.as_deref(),
            Some("remember to follow up")
        );

        // (Re-)transcription writes the transcript columns but must NOT touch notes.
        db.update_transcript(&r.id, "machine output", "machine output", "ggml-base")
            .await
            .expect("machine transcript");
        let after_transcribe = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(
            after_transcribe.transcript.as_deref(),
            Some("machine output")
        );
        assert_eq!(
            after_transcribe.notes.as_deref(),
            Some("remember to follow up"),
            "re-transcription must not clear notes"
        );

        // A manual transcript edit must also preserve notes.
        db.update_user_transcript(&r.id, "edited by the user")
            .await
            .expect("user edit");
        assert_eq!(
            db.get(&r.id).await.unwrap().unwrap().notes.as_deref(),
            Some("remember to follow up"),
            "user transcript edit must not clear notes"
        );
    }

    #[tokio::test]
    async fn set_title_auto_writes_never_overwrite_a_user_title() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();

        // Fresh rows are untitled and auto-owned (the migration default).
        let got = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(got.title, None);
        assert!(got.title_is_auto, "fresh rows must be auto-owned");

        // An auto write lands while the title is auto-owned (and a later auto
        // write — e.g. a retranscribe — refreshes it AND its recorded model).
        assert!(db
            .set_title(&r.id, Some("first pass"), true, Some("gemma3"))
            .await
            .unwrap());
        assert!(db
            .set_title(&r.id, Some("second pass"), true, Some("gemma3:4b"))
            .await
            .unwrap());
        let got = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(got.title.as_deref(), Some("second pass"));
        assert!(got.title_is_auto);
        assert_eq!(
            got.title_model.as_deref(),
            Some("gemma3:4b"),
            "an auto refresh records the new title model"
        );

        // The user takes ownership; from now on auto writes are no-ops, and the
        // stale auto-title model is cleared (a user title never shows one).
        assert!(db
            .set_title(&r.id, Some("My title"), false, None)
            .await
            .unwrap());
        assert!(
            !db.set_title(&r.id, Some("auto again"), true, Some("x"))
                .await
                .unwrap(),
            "an auto write must be skipped once the user owns the title"
        );
        let got = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(got.title.as_deref(), Some("My title"));
        assert!(!got.title_is_auto, "title_is_auto = 0 wins forever");
        assert_eq!(got.title_model, None, "a user title carries no model");

        // Clearing (None) empties the title and reverts ownership to auto, so
        // the next pipeline run may fill it again.
        assert!(db.set_title(&r.id, None, true, None).await.unwrap());
        let got = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(got.title, None);
        assert!(got.title_is_auto, "a cleared title reverts to auto-owned");
        assert!(db
            .set_title(&r.id, Some("fresh auto"), true, Some("llama3"))
            .await
            .unwrap());
        assert_eq!(
            db.get(&r.id).await.unwrap().unwrap().title.as_deref(),
            Some("fresh auto")
        );

        // Unknown ids report no update.
        assert!(!db
            .set_title(&RecordingId::new(), Some("x"), false, None)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn meeting_session_two_tracks_share_meeting_id_and_round_trip() {
        // Meeting Mode (v1.6): a meeting produces TWO recordings that share a
        // freshly-minted meeting_id and differ only by `track`. Both must
        // round-trip through insert/get/list, and a fresh single-track
        // recording must leave both columns NULL.
        let db = Catalog::open(Path::new("sqlite::memory:"))
            .await
            .expect("open db");

        let meeting_id = "meeting-abc123".to_string();
        let make = |track: &str| Recording {
            id: RecordingId::new(),
            started_at: Local::now(),
            duration_ms: 1000,
            audio_path: format!("{track}.wav"),
            transcript: None,
            model: None,
            status: RecordingStatus::Transcribing,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
            notes: None,
            meeting_id: Some(meeting_id.clone()),
            meeting_name: None,
            track: Some(track.to_string()),
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            tags: vec![],
            speaker_names: vec![],
        };
        let mic = make("mic");
        let system = make("system");
        db.insert(&mic).await.expect("insert mic");
        db.insert(&system).await.expect("insert system");

        // Each row round-trips with its meeting_id + track intact.
        let got_mic = db.get(&mic.id).await.unwrap().unwrap();
        let got_sys = db.get(&system.id).await.unwrap().unwrap();
        assert_eq!(got_mic.meeting_id.as_deref(), Some("meeting-abc123"));
        assert_eq!(got_mic.track.as_deref(), Some("mic"));
        assert_eq!(got_sys.meeting_id.as_deref(), Some("meeting-abc123"));
        assert_eq!(got_sys.track.as_deref(), Some("system"));

        // The two recordings share one meeting_id.
        assert_eq!(got_mic.meeting_id, got_sys.meeting_id);

        // A normal single-track recording leaves both columns NULL.
        let solo = Recording {
            id: RecordingId::new(),
            started_at: Local::now(),
            duration_ms: 1000,
            audio_path: "solo.wav".into(),
            transcript: None,
            model: None,
            status: RecordingStatus::Recording,
            error_kind: None,
            error_message: None,
            hook_command: None,
            hook_exit_code: None,
            hook_duration_ms: None,
            transcribed_at: None,
            hook_ran_at: None,
            notes: None,
            meeting_id: None,
            meeting_name: None,
            track: None,
            in_place: false,
            cleanup_model: None,
            diarized: false,
            user_edited: false,
            favorite: false,
            tag_suggestions: vec![],
            summary: None,
            summary_model: None,
            title: None,
            title_is_auto: true,
            title_model: None,
            tag_model: None,
            diarization_model: None,
            tags: vec![],
            speaker_names: vec![],
        };
        db.insert(&solo).await.expect("insert solo");
        let got_solo = db.get(&solo.id).await.unwrap().unwrap();
        assert_eq!(got_solo.meeting_id, None);
        assert_eq!(got_solo.track, None);

        // Both meeting rows are visible via list().
        let all = db.list(&ListFilter::default()).await.unwrap();
        let with_session: Vec<_> = all
            .iter()
            .filter(|r| r.meeting_id.as_deref() == Some("meeting-abc123"))
            .collect();
        assert_eq!(with_session.len(), 2, "both meeting tracks must be listed");
    }

    // ── Named speakers ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn speaker_names_set_get_rename_and_clear() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();

        // No names initially.
        assert!(db.speaker_names_for(&r.id).await.unwrap().is_empty());

        // Set two distinct speaker names; they come back ordered by index.
        db.set_speaker_name(&r.id, 1, "Sarah").await.unwrap();
        db.set_speaker_name(&r.id, 2, "Alex").await.unwrap();
        let names = db.speaker_names_for(&r.id).await.unwrap();
        assert_eq!(
            names,
            vec![
                SpeakerName {
                    speaker_label: 1,
                    name: "Sarah".into()
                },
                SpeakerName {
                    speaker_label: 2,
                    name: "Alex".into()
                },
            ]
        );

        // Re-setting the same label updates in place (upsert, not a duplicate row).
        db.set_speaker_name(&r.id, 1, "Sarah Connor").await.unwrap();
        let names = db.speaker_names_for(&r.id).await.unwrap();
        assert_eq!(names.len(), 2, "rename must not add a row");
        assert_eq!(names[0].name, "Sarah Connor");

        // Names are trimmed on the way in.
        db.set_speaker_name(&r.id, 2, "  Alex P.  ").await.unwrap();
        assert_eq!(
            db.speaker_names_for(&r.id).await.unwrap()[1].name,
            "Alex P."
        );

        // A blank/whitespace name clears the mapping (reverts to "Speaker N").
        db.set_speaker_name(&r.id, 1, "   ").await.unwrap();
        let names = db.speaker_names_for(&r.id).await.unwrap();
        assert_eq!(
            names,
            vec![SpeakerName {
                speaker_label: 2,
                name: "Alex P.".into()
            }],
            "clearing speaker 1 leaves only speaker 2"
        );
    }

    #[tokio::test]
    async fn set_speaker_name_if_absent_seeds_then_never_overwrites() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();

        // Absent → the friendly default is seeded (trimmed like set_speaker_name).
        db.set_speaker_name_if_absent(&r.id, 1, "  You  ")
            .await
            .unwrap();
        assert_eq!(
            db.speaker_names_for(&r.id).await.unwrap(),
            vec![SpeakerName {
                speaker_label: 1,
                name: "You".into()
            }]
        );

        // A blank name never seeds an empty default (no-op).
        db.set_speaker_name_if_absent(&r.id, 2, "   ")
            .await
            .unwrap();
        assert_eq!(
            db.speaker_names_for(&r.id).await.unwrap().len(),
            1,
            "blank name must not seed a row"
        );

        // Present → a later if-absent write is a no-op, preserving a user rename.
        db.set_speaker_name(&r.id, 1, "Alice").await.unwrap();
        db.set_speaker_name_if_absent(&r.id, 1, "You")
            .await
            .unwrap();
        assert_eq!(
            db.speaker_names_for(&r.id).await.unwrap(),
            vec![SpeakerName {
                speaker_label: 1,
                name: "Alice".into()
            }],
            "if-absent must NOT clobber an existing user rename"
        );
    }

    #[tokio::test]
    async fn speaker_names_are_populated_by_get_and_list() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();
        db.set_speaker_name(&r.id, 1, "Sarah").await.unwrap();

        // get() carries the speaker-name map (backs the detail view).
        let got = db.get(&r.id).await.unwrap().unwrap();
        assert_eq!(
            got.speaker_names,
            vec![SpeakerName {
                speaker_label: 1,
                name: "Sarah".into()
            }]
        );

        // list() carries it too.
        let listed = db.list(&ListFilter::default()).await.unwrap();
        let row = listed.iter().find(|x| x.id == r.id).unwrap();
        assert_eq!(row.speaker_names.len(), 1);
        assert_eq!(row.speaker_names[0].name, "Sarah");
    }

    #[tokio::test]
    async fn speaker_names_populated_per_track_by_list_by_meeting() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let mic = embedded_recording(Some("m-1"));
        let sys = embedded_recording(Some("m-1"));
        db.insert(&mic).await.unwrap();
        db.insert(&sys).await.unwrap();
        // Each track keeps its own per-recording speaker names.
        db.set_speaker_name(&mic.id, 1, "Me").await.unwrap();
        db.set_speaker_name(&sys.id, 1, "Caller").await.unwrap();

        let tracks = db.list_by_meeting("m-1").await.unwrap();
        assert_eq!(tracks.len(), 2);
        for t in &tracks {
            let expected = if t.id == mic.id { "Me" } else { "Caller" };
            assert_eq!(
                t.speaker_names,
                vec![SpeakerName {
                    speaker_label: 1,
                    name: expected.into()
                }]
            );
        }
    }

    #[tokio::test]
    async fn speaker_names_cascade_deleted_with_recording() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();
        db.set_speaker_name(&r.id, 1, "Sarah").await.unwrap();

        db.delete(&r.id).await.unwrap();
        // The FK ON DELETE CASCADE must drop the orphaned name rows.
        assert!(
            db.speaker_names_for(&r.id).await.unwrap().is_empty(),
            "speaker names must be cascade-deleted with their recording"
        );
    }

    #[tokio::test]
    async fn retention_audio_only_keeps_rows_and_is_idempotent() {
        // delete_audio = true: the WAV path is returned for deletion and
        // blanked on the row, but the row itself (transcript, metadata)
        // SURVIVES — and a second sweep finds nothing left to reclaim.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let mut r = embedded_recording(None);
        r.started_at = Local::now() - chrono::Duration::days(90);
        db.insert(&r).await.unwrap();

        let cfg = crate::config::RetentionConfig {
            max_age_days: Some(30),
            max_count: None,
            delete_audio: true,
        };
        let paths = db.apply_retention(&cfg).await.unwrap();
        assert_eq!(paths, vec!["x.wav".to_string()]);

        let row = db.get(&r.id).await.unwrap().expect("row must survive");
        assert_eq!(row.audio_path, "", "audio path blanked after reclaim");
        assert_eq!(row.transcript.as_deref(), Some("t"), "transcript kept");

        let again = db.apply_retention(&cfg).await.unwrap();
        assert!(again.is_empty(), "second sweep must be a no-op");
    }

    #[tokio::test]
    async fn retention_default_deletes_row_and_audio_together() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let mut r = embedded_recording(None);
        r.started_at = Local::now() - chrono::Duration::days(90);
        db.insert(&r).await.unwrap();

        let cfg = crate::config::RetentionConfig {
            max_age_days: Some(30),
            max_count: None,
            delete_audio: false,
        };
        let paths = db.apply_retention(&cfg).await.unwrap();
        assert_eq!(paths.len(), 1);
        assert!(db.get(&r.id).await.unwrap().is_none(), "row deleted");
    }

    #[tokio::test]
    async fn clear_all_tag_suggestions_sweeps_every_recording() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let a = embedded_recording(None);
        let b = embedded_recording(None);
        let c = embedded_recording(None); // never had suggestions
        db.insert(&a).await.unwrap();
        db.insert(&b).await.unwrap();
        db.insert(&c).await.unwrap();
        db.set_tag_suggestions(&a.id, &["alpha".into()])
            .await
            .unwrap();
        db.set_tag_suggestions(&b.id, &["beta".into(), "gamma".into()])
            .await
            .unwrap();

        let cleared = db.clear_all_tag_suggestions().await.unwrap();
        assert_eq!(cleared, 2, "only rows that HAD suggestions count");
        for id in [&a.id, &b.id, &c.id] {
            let rec = db.get(id).await.unwrap().unwrap();
            assert!(rec.tag_suggestions.is_empty());
        }
        // Sweep again: nothing left to clear.
        assert_eq!(db.clear_all_tag_suggestions().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn add_tag_is_case_insensitive() {
        // "Code" and "code" are the same tag: the second add must reuse the
        // first row (same id, its casing, its color) instead of minting a
        // byte-wise-unique duplicate.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let first = db.add_tag("Code", Some("#f00")).await.unwrap();
        let second = db.add_tag("code", None).await.unwrap();
        assert_eq!(first.id, second.id, "casing variants must reuse the tag");
        assert_eq!(second.name, "Code", "the first-created casing wins");
        assert_eq!(second.color.as_deref(), Some("#f00"), "existing color kept");
    }

    #[tokio::test]
    async fn segments_replace_round_trip_and_cascade() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();

        // No segments yet is a normal (empty) state, not an error.
        assert!(db.segments_for(&r.id).await.unwrap().is_empty());

        let first = vec![
            TranscriptSegment {
                start_ms: 0,
                end_ms: 1200,
                text: "hello".into(),
                speaker: Some("1".into()),
            },
            TranscriptSegment {
                start_ms: 1200,
                end_ms: 2500,
                text: "hi there".into(),
                speaker: Some("2".into()),
            },
        ];
        db.replace_segments(&r.id, &first).await.unwrap();
        assert_eq!(db.segments_for(&r.id).await.unwrap(), first);

        // A retranscribe REPLACES the timeline — fewer rows must not leave
        // stale tail segments behind.
        let second = vec![TranscriptSegment {
            start_ms: 0,
            end_ms: 900,
            text: "rerun".into(),
            speaker: None,
        }];
        db.replace_segments(&r.id, &second).await.unwrap();
        assert_eq!(db.segments_for(&r.id).await.unwrap(), second);

        db.delete(&r.id).await.unwrap();
        assert!(
            db.segments_for(&r.id).await.unwrap().is_empty(),
            "segments must be cascade-deleted with their recording"
        );
    }

    #[tokio::test]
    async fn words_replace_round_trip_and_cascade() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();

        // No words yet is a normal (empty) state, not an error.
        assert!(db.words_for(&r.id).await.unwrap().is_empty());

        // A mix of present and absent confidence — the nullable column must
        // round-trip both (a whisper-family word has `None`, a Deepgram word
        // has a score). Ordering by idx is the timeline order.
        let first = vec![
            TranscriptWord {
                start_ms: 0,
                end_ms: 400,
                text: "hello".into(),
                leading_space: true,
                speaker: Some("1".into()),
                confidence: Some(0.97),
            },
            TranscriptWord {
                start_ms: 400,
                end_ms: 900,
                text: "there".into(),
                leading_space: true,
                speaker: Some("1".into()),
                confidence: None,
            },
        ];
        db.replace_words(&r.id, &first).await.unwrap();
        let got = db.words_for(&r.id).await.unwrap();
        assert_eq!(got, first, "words round-trip in idx order, confidence kept");
        assert_eq!(got[0].confidence, Some(0.97));
        assert_eq!(got[1].confidence, None, "a NULL confidence stays None");

        // A retranscribe REPLACES the word timeline — fewer rows must not leave
        // stale tail words behind.
        let second = vec![TranscriptWord {
            start_ms: 0,
            end_ms: 500,
            text: "rerun".into(),
            leading_space: false, // a continuation token — must round-trip as false
            speaker: None,
            confidence: Some(0.5),
        }];
        db.replace_words(&r.id, &second).await.unwrap();
        assert_eq!(db.words_for(&r.id).await.unwrap(), second);

        db.delete(&r.id).await.unwrap();
        assert!(
            db.words_for(&r.id).await.unwrap().is_empty(),
            "words must be cascade-deleted with their recording"
        );
    }

    // ── In-recording speaker correction (U1) ───────────────────────────────

    /// Read the prose transcript text straight from the row.
    async fn transcript_text(db: &Catalog, id: &RecordingId) -> Option<String> {
        sqlx::query_scalar::<_, Option<String>>("SELECT transcript FROM recordings WHERE id = ?")
            .bind(id.as_str())
            .fetch_one(&db.pool)
            .await
            .unwrap()
    }

    fn seg(start_ms: i64, end_ms: i64, text: &str, speaker: &str) -> TranscriptSegment {
        TranscriptSegment {
            start_ms,
            end_ms,
            text: text.into(),
            speaker: Some(speaker.into()),
        }
    }

    /// Seed a two-speaker diarized recording: segments, the matching prose
    /// transcript, and one word per segment.
    async fn seed_diarized(db: &Catalog) -> Recording {
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();
        let segs = vec![
            seg(0, 1000, "hello there", "1"),
            seg(1000, 2000, "hi yourself", "2"),
            seg(2000, 3000, "how are you", "1"),
        ];
        db.replace_segments(&r.id, &segs).await.unwrap();
        let words = vec![
            TranscriptWord {
                start_ms: 0,
                end_ms: 1000,
                text: "hello there".into(),
                leading_space: true,
                speaker: Some("1".into()),
                confidence: None,
            },
            TranscriptWord {
                start_ms: 1000,
                end_ms: 2000,
                text: "hi yourself".into(),
                leading_space: true,
                speaker: Some("2".into()),
                confidence: None,
            },
            TranscriptWord {
                start_ms: 2000,
                end_ms: 3000,
                text: "how are you".into(),
                leading_space: true,
                speaker: Some("1".into()),
                confidence: None,
            },
        ];
        db.replace_words(&r.id, &words).await.unwrap();
        db.update_transcript(
            &r.id,
            "[Speaker 1]: hello there\n\n[Speaker 2]: hi yourself\n\n[Speaker 1]: how are you",
            "orig",
            "tiny",
        )
        .await
        .unwrap();
        r
    }

    fn labels(segs: &[TranscriptSegment]) -> Vec<Option<&str>> {
        segs.iter().map(|s| s.speaker.as_deref()).collect()
    }

    #[tokio::test]
    async fn reassign_segment_moves_to_existing_and_brand_new_label() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = seed_diarized(&db).await;

        // Move the middle segment (idx 1) from speaker 2 → speaker 1.
        db.reassign_segment(&r.id, 1, 1).await.unwrap();
        let segs = db.segments_for(&r.id).await.unwrap();
        assert_eq!(labels(&segs), vec![Some("1"), Some("1"), Some("1")]);
        // All three turns now coalesce under one speaker in the prose markers.
        assert_eq!(
            transcript_text(&db, &r.id).await.as_deref(),
            Some("[Speaker 1]: hello there hi yourself how are you"),
            "consecutive same-label segments coalesce into one turn"
        );
        // The word layer for that segment's span followed.
        let words = db.words_for(&r.id).await.unwrap();
        assert_eq!(words[1].speaker.as_deref(), Some("1"));

        // Reassign the same segment to a brand-new label 3 — it simply starts.
        db.reassign_segment(&r.id, 1, 3).await.unwrap();
        let segs = db.segments_for(&r.id).await.unwrap();
        assert_eq!(labels(&segs), vec![Some("1"), Some("3"), Some("1")]);
        assert_eq!(
            transcript_text(&db, &r.id).await.as_deref(),
            Some(
                "[Speaker 1]: hello there\n\n[Speaker 3]: hi yourself\n\n[Speaker 1]: how are you"
            ),
        );
    }

    #[tokio::test]
    async fn reassign_segment_unknown_idx_errors_with_no_mutation() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = seed_diarized(&db).await;
        let before = db.segments_for(&r.id).await.unwrap();
        let before_text = transcript_text(&db, &r.id).await;

        let err = db.reassign_segment(&r.id, 99, 2).await.unwrap_err();
        assert!(matches!(err, crate::error::Error::NotFound { .. }));
        assert_eq!(
            db.segments_for(&r.id).await.unwrap(),
            before,
            "no segment write"
        );
        assert_eq!(
            transcript_text(&db, &r.id).await,
            before_text,
            "no text write"
        );

        // A label < 1 is rejected before any write too.
        assert!(db.reassign_segment(&r.id, 0, 0).await.is_err());
        assert_eq!(db.segments_for(&r.id).await.unwrap(), before);
    }

    #[tokio::test]
    async fn merge_speakers_repoints_segments_and_keeps_into_name() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = seed_diarized(&db).await;
        db.set_speaker_name(&r.id, 1, "Ada").await.unwrap();
        db.set_speaker_name(&r.id, 2, "Bob").await.unwrap();

        // Merge speaker 2 into speaker 1: every 2-segment becomes 1.
        db.merge_speakers(&r.id, 2, 1).await.unwrap();
        let segs = db.segments_for(&r.id).await.unwrap();
        assert_eq!(labels(&segs), vec![Some("1"), Some("1"), Some("1")]);
        // `into` (1) keeps its name; `from` (2) name row is gone.
        let names = db.speaker_names_for(&r.id).await.unwrap();
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].speaker_label, 1);
        assert_eq!(names[0].name, "Ada", "into keeps its own name");
        // Prose markers reflect the merge.
        assert_eq!(
            transcript_text(&db, &r.id).await.as_deref(),
            Some("[Speaker 1]: hello there hi yourself how are you"),
        );
        // Words followed too.
        let words = db.words_for(&r.id).await.unwrap();
        assert!(words.iter().all(|w| w.speaker.as_deref() == Some("1")));
    }

    #[tokio::test]
    async fn merge_speakers_into_adopts_froms_name_when_into_unnamed() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = seed_diarized(&db).await;
        // Only `from` (2) is named; `into` (1) is not.
        db.set_speaker_name(&r.id, 2, "Bob").await.unwrap();

        db.merge_speakers(&r.id, 2, 1).await.unwrap();
        let names = db.speaker_names_for(&r.id).await.unwrap();
        assert_eq!(
            names,
            vec![SpeakerName {
                speaker_label: 1,
                name: "Bob".into()
            }],
            "an unnamed into adopts from's name; the from row is removed"
        );
    }

    #[tokio::test]
    async fn merge_speakers_drops_froms_voiceprint_and_recomputes_library() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = seed_diarized(&db).await;
        // Both labels captured and enrolled under DIFFERENT named voices.
        db.save_speaker_voiceprint(r.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        db.save_speaker_voiceprint(r.id.as_str(), 2, &[0.0, 1.0, 0.0], 0)
            .await
            .unwrap();
        let ada = db
            .enroll_speaker(r.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();
        let bob = db
            .enroll_speaker(r.id.as_str(), 2, "Bob")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(named_voice_samples(&db, &bob).await, 1);

        // Merge 2 → 1: from's (2) capture is dropped; Bob's library entry
        // recomputes to zero linked samples.
        db.merge_speakers(&r.id, 2, 1).await.unwrap();
        assert!(
            db.speaker_voiceprint(r.id.as_str(), 2)
                .await
                .unwrap()
                .is_none(),
            "from's capture row is deleted"
        );
        assert!(
            db.speaker_voiceprint(r.id.as_str(), 1)
                .await
                .unwrap()
                .is_some(),
            "into's capture is untouched"
        );
        assert_eq!(
            named_voice_samples(&db, &bob).await,
            0,
            "Bob no longer counts the dropped capture"
        );
        assert_eq!(named_voice_samples(&db, &ada).await, 1, "Ada untouched");
    }

    #[tokio::test]
    async fn merge_speakers_unknown_from_errors_with_no_mutation() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = seed_diarized(&db).await;
        let before = db.segments_for(&r.id).await.unwrap();

        // No segment carries label 5.
        let err = db.merge_speakers(&r.id, 5, 1).await.unwrap_err();
        assert!(matches!(err, crate::error::Error::NotFound { .. }));
        assert_eq!(db.segments_for(&r.id).await.unwrap(), before);

        // Self-merge and bad labels are rejected.
        assert!(db.merge_speakers(&r.id, 1, 1).await.is_err());
        assert!(db.merge_speakers(&r.id, 0, 1).await.is_err());
        assert_eq!(db.segments_for(&r.id).await.unwrap(), before);
    }

    #[tokio::test]
    async fn split_speaker_moves_listed_segments_only() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = seed_diarized(&db).await;
        // Speaker 1 owns segments 0 and 2. Split segment 2 off onto a fresh 3.
        db.split_speaker(&r.id, 1, &[2], 3).await.unwrap();
        let segs = db.segments_for(&r.id).await.unwrap();
        assert_eq!(labels(&segs), vec![Some("1"), Some("2"), Some("3")]);
        assert_eq!(
            transcript_text(&db, &r.id).await.as_deref(),
            Some(
                "[Speaker 1]: hello there\n\n[Speaker 2]: hi yourself\n\n[Speaker 3]: how are you"
            ),
        );
        // The new label has no name and no voiceprint until enrolled.
        assert!(db
            .speaker_names_for(&r.id)
            .await
            .unwrap()
            .iter()
            .all(|n| n.speaker_label != 3));
        assert!(db
            .speaker_voiceprint(r.id.as_str(), 3)
            .await
            .unwrap()
            .is_none());
        // The word in segment 2's span followed.
        let words = db.words_for(&r.id).await.unwrap();
        assert_eq!(words[2].speaker.as_deref(), Some("3"));
    }

    #[tokio::test]
    async fn split_speaker_rejects_idx_not_owned_by_label_with_no_mutation() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = seed_diarized(&db).await;
        let before = db.segments_for(&r.id).await.unwrap();

        // Segment 1 belongs to speaker 2, not 1 — the whole op must abort.
        let err = db.split_speaker(&r.id, 1, &[0, 1], 3).await.unwrap_err();
        assert!(matches!(err, crate::error::Error::Internal(_)));
        assert_eq!(
            db.segments_for(&r.id).await.unwrap(),
            before,
            "one bad idx rolls back the whole split (segment 0 not moved)"
        );

        // Unknown idx, empty list, self-target, bad labels all error cleanly.
        assert!(matches!(
            db.split_speaker(&r.id, 1, &[99], 3).await.unwrap_err(),
            crate::error::Error::NotFound { .. }
        ));
        assert!(db.split_speaker(&r.id, 1, &[], 3).await.is_err());
        assert!(db.split_speaker(&r.id, 1, &[0], 1).await.is_err());
        assert_eq!(db.segments_for(&r.id).await.unwrap(), before);
    }

    #[tokio::test]
    async fn speaker_edit_leaves_plain_transcript_markers_untouched() {
        // A non-diarized transcript has no `[Speaker N]` markers to keep
        // consistent — the rebuild must leave the prose text alone (segments
        // still update, driving the timeline views).
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let r = embedded_recording(None);
        db.insert(&r).await.unwrap();
        db.replace_segments(&r.id, &[seg(0, 1000, "just one voice", "1")])
            .await
            .unwrap();
        db.update_transcript(&r.id, "just one voice", "orig", "tiny")
            .await
            .unwrap();

        db.reassign_segment(&r.id, 0, 2).await.unwrap();
        assert_eq!(
            transcript_text(&db, &r.id).await.as_deref(),
            Some("just one voice"),
            "plain prose is never marker-rewritten"
        );
        assert_eq!(
            db.segments_for(&r.id).await.unwrap()[0].speaker.as_deref(),
            Some("2"),
            "the segment label still updates"
        );
    }

    /// One recording's sample count for a named voice, read straight from the
    /// library row (the cached count `recompute_named_centroid` maintains).
    async fn named_voice_samples(db: &Catalog, id: &str) -> i64 {
        sqlx::query_scalar("SELECT samples FROM named_voiceprints WHERE id = ?")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn recognize_speakers_one_to_one_never_doubles_a_name() {
        // Audit H2: two captured speakers in ONE recording both nearest the same
        // library voice must NOT both be suggested that name — at most one gets
        // it, the other is left unknown.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

        // Enroll one library voice "Ada" from a source recording.
        let src = embedded_recording(None);
        db.insert(&src).await.unwrap();
        db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        let ada = db
            .enroll_speaker(src.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();

        // A second recording with two captured speakers, both closest to Ada
        // (one a near-perfect match, the other a clear runner-up).
        let rec = embedded_recording(None);
        db.insert(&rec).await.unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 2, &[0.95, 0.31, 0.0], 0)
            .await
            .unwrap();

        let sugg = db
            .recognize_speakers_for(rec.id.as_str(), 0.5, crate::voiceprint::ScoreNorm::Off)
            .await
            .unwrap();
        let to_ada: Vec<_> = sugg.iter().filter(|s| s.named_voice_id == ada).collect();
        assert_eq!(
            to_ada.len(),
            1,
            "a name can be suggested to at most one speaker per recording"
        );
        assert_eq!(
            to_ada[0].speaker_label, 1,
            "the closest speaker wins the name"
        );
        assert!(
            sugg.len() <= 1,
            "only one library voice exists, so at most one suggestion"
        );
    }

    #[tokio::test]
    async fn recognize_speakers_assigns_distinct_voices_one_each() {
        // Two library voices, two speakers each clearly nearest a different one:
        // both get their own name, no crossover.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let src = embedded_recording(None);
        db.insert(&src).await.unwrap();
        db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        db.save_speaker_voiceprint(src.id.as_str(), 2, &[0.0, 1.0, 0.0], 0)
            .await
            .unwrap();
        let ada = db
            .enroll_speaker(src.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();
        let bob = db
            .enroll_speaker(src.id.as_str(), 2, "Bob")
            .await
            .unwrap()
            .unwrap();

        let rec = embedded_recording(None);
        db.insert(&rec).await.unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 2, &[0.0, 1.0, 0.0], 0)
            .await
            .unwrap();

        let sugg = db
            .recognize_speakers_for(rec.id.as_str(), 0.5, crate::voiceprint::ScoreNorm::Off)
            .await
            .unwrap();
        assert_eq!(sugg.len(), 2, "both speakers recognized");
        assert_eq!(sugg[0].speaker_label, 1, "sorted by speaker_label");
        assert_eq!(sugg[0].named_voice_id, ada);
        assert_eq!(sugg[1].named_voice_id, bob);
    }

    #[tokio::test]
    async fn recognize_speakers_skips_ambiguous_speaker() {
        // A speaker nearly equidistant from two library voices (within MARGIN)
        // is too ambiguous to name — no suggestion is emitted for it.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let src = embedded_recording(None);
        db.insert(&src).await.unwrap();
        db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0], 0)
            .await
            .unwrap();
        db.save_speaker_voiceprint(src.id.as_str(), 2, &[0.0, 1.0], 0)
            .await
            .unwrap();
        db.enroll_speaker(src.id.as_str(), 1, "Ada").await.unwrap();
        db.enroll_speaker(src.id.as_str(), 2, "Bob").await.unwrap();

        // Probe at 45°: cosine ~0.707 to both voices — tie inside MARGIN.
        let rec = embedded_recording(None);
        db.insert(&rec).await.unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 1.0], 0)
            .await
            .unwrap();
        let sugg = db
            .recognize_speakers_for(rec.id.as_str(), 0.5, crate::voiceprint::ScoreNorm::Off)
            .await
            .unwrap();
        assert!(sugg.is_empty(), "an ambiguous speaker is left unknown");
    }

    #[tokio::test]
    async fn recognize_speakers_off_mode_unchanged_from_raw_assign() {
        // V2 default-off contract at the catalog layer: routing through the new
        // mode argument with ScoreNorm::Off produces the same assignment as the
        // raw assign_speakers it delegates to.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let src = embedded_recording(None);
        db.insert(&src).await.unwrap();
        db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        db.save_speaker_voiceprint(src.id.as_str(), 2, &[0.0, 1.0, 0.0], 0)
            .await
            .unwrap();
        let ada = db
            .enroll_speaker(src.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();
        let bob = db
            .enroll_speaker(src.id.as_str(), 2, "Bob")
            .await
            .unwrap()
            .unwrap();

        let rec = embedded_recording(None);
        db.insert(&rec).await.unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 1, &[0.98, 0.02, 0.0], 0)
            .await
            .unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 2, &[0.02, 0.98, 0.0], 0)
            .await
            .unwrap();

        let off = db
            .recognize_speakers_for(rec.id.as_str(), 0.5, crate::voiceprint::ScoreNorm::Off)
            .await
            .unwrap();
        assert_eq!(off.len(), 2);
        assert_eq!(off[0].speaker_label, 1);
        assert_eq!(off[0].named_voice_id, ada);
        assert_eq!(off[1].named_voice_id, bob);
        // Off scores are still raw cosines (≈1), not z-scores.
        assert!(off[0].score > 0.9 && off[1].score > 0.9, "{off:?}");
    }

    #[tokio::test]
    async fn recognize_speakers_snorm_routes_and_assigns() {
        // V2 on-path smoke test: with S-norm the scores are z-scores (can exceed
        // 1 / be negative), so the threshold is a z-bar. The two clearly-distinct
        // speakers still each get their own name. Cohort = the 3 library voices.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let src = embedded_recording(None);
        db.insert(&src).await.unwrap();
        // Three enrolled voices spread out so each probe has a real cohort.
        for (label, v) in [
            (1i64, [1.0f32, 0.0, 0.0]),
            (2, [0.0, 1.0, 0.0]),
            (3, [0.0, 0.0, 1.0]),
        ] {
            db.save_speaker_voiceprint(src.id.as_str(), label, &v, 0)
                .await
                .unwrap();
        }
        let ada = db
            .enroll_speaker(src.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();
        let bob = db
            .enroll_speaker(src.id.as_str(), 2, "Bob")
            .await
            .unwrap()
            .unwrap();
        db.enroll_speaker(src.id.as_str(), 3, "Cleo").await.unwrap();

        let rec = embedded_recording(None);
        db.insert(&rec).await.unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 1, &[0.99, 0.01, 0.0], 0) // ~Ada
            .await
            .unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 2, &[0.01, 0.99, 0.0], 0) // ~Bob
            .await
            .unwrap();

        // z-bar ~1.0: a genuine match stands well above its cohort mean.
        let sugg = db
            .recognize_speakers_for(rec.id.as_str(), 1.0, crate::voiceprint::ScoreNorm::SNorm)
            .await
            .unwrap();
        assert_eq!(sugg.len(), 2, "both distinct speakers recognized: {sugg:?}");
        assert_eq!(sugg[0].speaker_label, 1);
        assert_eq!(sugg[0].named_voice_id, ada);
        assert_eq!(sugg[1].named_voice_id, bob);
    }

    #[tokio::test]
    async fn enroll_speaker_clears_a_prior_dismissal() {
        // Audit M9: a speaker dismissed before the right voice existed must be
        // recognizable once it's named — naming clears the dismissal row.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let rec = embedded_recording(None);
        db.insert(&rec).await.unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        db.dismiss_speaker_suggestion(rec.id.as_str(), 1)
            .await
            .unwrap();

        // Naming the speaker is an explicit ID — the dismissal must be gone.
        db.enroll_speaker(rec.id.as_str(), 1, "Ada").await.unwrap();
        let still_dismissed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM dismissed_speaker_suggestions \
             WHERE recording_id = ? AND speaker_label = ?",
        )
        .bind(rec.id.as_str())
        .bind(1i64)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(still_dismissed, 0, "enrolling clears the dismissal");
    }

    #[tokio::test]
    async fn recompute_named_centroid_drops_an_outlier_capture() {
        // Audit M7: with >= 4 captures, a clear wrong-speaker outlier is pruned
        // before the mean, so the cached sample count drops and the centroid
        // stays close to the genuine cluster.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let rec = embedded_recording(None);
        db.insert(&rec).await.unwrap();
        // Three tightly-clustered captures plus one clear (opposite-direction)
        // outlier — a wrong-speaker capture mistakenly named into this voice.
        db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 2, &[0.99, 0.10, 0.0], 0)
            .await
            .unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 3, &[0.98, 0.0, 0.10], 0)
            .await
            .unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 4, &[-1.0, 0.0, 0.0], 0) // outlier
            .await
            .unwrap();
        let ada = db
            .enroll_speaker(rec.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();
        for label in [2i64, 3, 4] {
            db.enroll_speaker(rec.id.as_str(), label, "Ada")
                .await
                .unwrap();
        }

        // Four captures linked, but the outlier is pruned → 3 effective samples.
        assert_eq!(
            named_voice_samples(&db, &ada).await,
            3,
            "the orthogonal outlier is dropped from the template"
        );
        // The surviving centroid still points at the cluster (cosine ~1 to it).
        let probe = vec![1.0f32, 0.0, 0.0];
        let (_, score) = db.recognize_voice(&probe, 0.5).await.unwrap().unwrap();
        assert!(
            score > 0.95,
            "centroid stays on the genuine cluster: {score}"
        );
    }

    #[tokio::test]
    async fn recompute_named_centroid_keeps_all_below_four_samples() {
        // Below 4 captures every sample counts — even a far one is kept (too few
        // to tell signal from noise).
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let rec = embedded_recording(None);
        db.insert(&rec).await.unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0], 0)
            .await
            .unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 2, &[0.0, 1.0], 0)
            .await
            .unwrap();
        let ada = db
            .enroll_speaker(rec.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();
        db.enroll_speaker(rec.id.as_str(), 2, "Ada").await.unwrap();
        assert_eq!(
            named_voice_samples(&db, &ada).await,
            2,
            "under the threshold no pruning happens"
        );
    }

    /// The named voice's cached centroid, read straight from the library row.
    async fn named_voice_centroid(db: &Catalog, id: &str) -> Vec<f32> {
        let json: String =
            sqlx::query_scalar("SELECT centroid FROM named_voiceprints WHERE id = ?")
                .bind(id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        serde_json::from_str(&json).unwrap()
    }

    #[tokio::test]
    async fn recompute_named_centroid_legacy_zero_durations_match_plain_mean() {
        // Backward-compat contract: a library whose captures all have
        // duration_ms=0 (built before V4) must recompute to the SAME centroid the
        // old unweighted mean produced — i.e. mean_centroid of those vectors.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let rec = embedded_recording(None);
        db.insert(&rec).await.unwrap();
        let vecs = [
            vec![1.0f32, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.7, 0.7, 0.0],
        ];
        for (i, v) in vecs.iter().enumerate() {
            db.save_speaker_voiceprint(rec.id.as_str(), (i + 1) as i64, v, 0)
                .await
                .unwrap();
        }
        let ada = db
            .enroll_speaker(rec.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();
        db.enroll_speaker(rec.id.as_str(), 2, "Ada").await.unwrap();
        db.enroll_speaker(rec.id.as_str(), 3, "Ada").await.unwrap();

        // Below the 4-sample pruning threshold, so all three survive → the cached
        // centroid is exactly the unweighted mean of the three vectors.
        let expected = crate::voiceprint::mean_centroid(&vecs).unwrap();
        let got = named_voice_centroid(&db, &ada).await;
        assert_eq!(got.len(), expected.len());
        for (g, e) in got.iter().zip(expected.iter()) {
            assert!(
                (g - e).abs() < 1e-6,
                "legacy recompute drifted: {got:?} vs {expected:?}"
            );
        }
    }

    #[tokio::test]
    async fn recompute_named_centroid_weights_toward_the_longer_capture() {
        // Two orthogonal captures of very different speaking duration. The plain
        // (unweighted) mean would sit at 45° — equidistant. Duration-weighting
        // must pull the cached centroid toward the much longer capture, so it
        // scores clearly higher to the long sample's direction than the short.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let rec = embedded_recording(None);
        db.insert(&rec).await.unwrap();
        let long_dir = [1.0f32, 0.0]; // spoke for minutes
        let short_dir = [0.0f32, 1.0]; // spoke for a moment
        db.save_speaker_voiceprint(rec.id.as_str(), 1, &long_dir, 600_000)
            .await
            .unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 2, &short_dir, 2_000)
            .await
            .unwrap();
        let ada = db
            .enroll_speaker(rec.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();
        db.enroll_speaker(rec.id.as_str(), 2, "Ada").await.unwrap();

        let centroid = named_voice_centroid(&db, &ada).await;
        let to_long = crate::voiceprint::cosine_similarity(&centroid, &long_dir);
        let to_short = crate::voiceprint::cosine_similarity(&centroid, &short_dir);
        assert!(
            to_long > to_short,
            "centroid must lean toward the longer capture: long {to_long} vs short {to_short}"
        );
        // And strictly past the unweighted-mean midpoint (cos 0.707 to each).
        let midpoint = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            to_long > midpoint + 1e-3,
            "weighting must move the centroid past the equal-weight midpoint: {to_long}"
        );
    }

    #[tokio::test]
    async fn recognize_voice_skips_dimension_mismatched_library() {
        // Audit L: a library centroid of a different dimension than the probe is
        // skipped (it came from another embedding model) — not silently scored 0.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let rec = embedded_recording(None);
        db.insert(&rec).await.unwrap();
        // Library voice has a 3-dim centroid.
        db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        db.enroll_speaker(rec.id.as_str(), 1, "Ada").await.unwrap();

        // Probe a 2-dim vector: dimension mismatch → skipped → no match.
        let got = db.recognize_voice(&[1.0, 0.0], 0.5).await.unwrap();
        assert!(
            got.is_none(),
            "a cross-model library entry is skipped, not matched at 0.0"
        );
    }

    #[tokio::test]
    async fn clear_all_recordings_recomputes_emptied_named_voices() {
        // Audit M1: wiping every recording cascades the voiceprints away, so the
        // named voice must drop to zero samples (not keep a stale count).
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let rec = embedded_recording(None);
        db.insert(&rec).await.unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        let ada = db
            .enroll_speaker(rec.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(named_voice_samples(&db, &ada).await, 1);

        db.clear_all_recordings().await.unwrap();
        assert_eq!(
            named_voice_samples(&db, &ada).await,
            0,
            "the library entry's count must follow its lost captures"
        );
    }

    #[tokio::test]
    async fn retention_hard_delete_recomputes_named_voices() {
        // Audit M1: a hard-delete retention sweep cascades the deleted recording's
        // voiceprints away, so its named voice's cached count must be recomputed.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        // Old recording (eligible) enrolled into "Ada".
        let old = {
            let mut r = embedded_recording(None);
            r.started_at = Local::now() - chrono::Duration::days(90);
            r
        };
        db.insert(&old).await.unwrap();
        db.save_speaker_voiceprint(old.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        let ada = db
            .enroll_speaker(old.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(named_voice_samples(&db, &ada).await, 1);

        let cfg = crate::config::RetentionConfig {
            max_age_days: Some(30),
            max_count: None,
            delete_audio: false,
        };
        db.apply_retention(&cfg).await.unwrap();
        assert!(db.get(&old.id).await.unwrap().is_none(), "old row swept");
        assert_eq!(
            named_voice_samples(&db, &ada).await,
            0,
            "the swept recording's named voice must lose its sample"
        );
    }

    // ---- Name propagation (V5) ----------------------------------------------

    /// The custom display name for a (recording, label), or None.
    async fn display_name(db: &Catalog, rid: &RecordingId, label: i64) -> Option<String> {
        db.speaker_names_for(rid)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.speaker_label == label)
            .map(|s| s.name)
    }

    /// Enroll "Ada" in a source recording, and create two more recordings that
    /// each have one unnamed speaker matching Ada and (in `rec_named`) one
    /// already-named "Bob". Returns `(db, ada_id, rec_match, rec_named, far)`.
    async fn propagation_fixture() -> (Catalog, String, RecordingId, RecordingId, RecordingId) {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();

        // Source: enroll Ada at [1,0,0].
        let src = embedded_recording(None);
        db.insert(&src).await.unwrap();
        db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        let ada = db
            .enroll_speaker(src.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();

        // rec_match: one UNNAMED speaker that matches Ada.
        let rec_match = embedded_recording(None);
        db.insert(&rec_match).await.unwrap();
        db.save_speaker_voiceprint(rec_match.id.as_str(), 1, &[0.99, 0.01, 0.0], 0)
            .await
            .unwrap();

        // rec_named: an unnamed Ada-match at label 1 AND an already-named "Bob"
        // (also near Ada geometrically — its name must NOT be overwritten).
        let rec_named = embedded_recording(None);
        db.insert(&rec_named).await.unwrap();
        db.save_speaker_voiceprint(rec_named.id.as_str(), 1, &[0.98, 0.0, 0.0], 0)
            .await
            .unwrap();
        db.save_speaker_voiceprint(rec_named.id.as_str(), 2, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        db.set_speaker_name(&rec_named.id, 2, "Bob").await.unwrap();

        // far: a dissimilar voice that must never be a candidate.
        let far = embedded_recording(None);
        db.insert(&far).await.unwrap();
        db.save_speaker_voiceprint(far.id.as_str(), 1, &[0.0, 0.0, 1.0], 0)
            .await
            .unwrap();

        (db, ada, rec_match.id, rec_named.id, far.id)
    }

    #[tokio::test]
    async fn propagation_candidates_respect_threshold_and_unnamed_only() {
        let (db, ada, rec_match, rec_named, far) = propagation_fixture().await;
        let cands = db
            .propagation_candidates(&ada, 0.5, crate::voiceprint::ScoreNorm::Off)
            .await
            .unwrap();

        // The two unnamed Ada-matches are candidates; the dissimilar "far" voice
        // is below threshold, and the already-named "Bob" (label 2 in rec_named)
        // is excluded because only `named_voice_id IS NULL` rows are scanned.
        let pairs: std::collections::HashSet<(String, i64)> = cands
            .iter()
            .map(|c| (c.recording_id.as_str().to_string(), c.speaker_label))
            .collect();
        assert!(pairs.contains(&(rec_match.as_str().to_string(), 1)));
        assert!(pairs.contains(&(rec_named.as_str().to_string(), 1)));
        assert!(
            !pairs.contains(&(rec_named.as_str().to_string(), 2)),
            "an already-named speaker is never a candidate"
        );
        assert!(
            !pairs.iter().any(|(rid, _)| rid == far.as_str()),
            "a dissimilar voice (below threshold) is not a candidate"
        );
        // The source recording (already enrolled under Ada) is excluded.
        assert_eq!(cands.len(), 2);
        // Ordered by score, highest first.
        assert!(cands[0].score >= cands[1].score);
    }

    #[tokio::test]
    async fn propagation_off_policy_backfills_nothing() {
        // The OFF policy is purely a routing decision in the IPC layer, but the
        // core invariant it relies on: candidates exist, yet nothing is applied
        // unless apply_propagation is called. Prove naming alone never touches
        // another recording (the Ask default proof below covers the live path).
        let (db, ada, rec_match, _rec_named, _far) = propagation_fixture().await;
        // Simulate OFF: gather candidates (or not) but never apply.
        let _ = db
            .propagation_candidates(&ada, 0.5, crate::voiceprint::ScoreNorm::Off)
            .await
            .unwrap();
        assert!(
            display_name(&db, &rec_match, 1).await.is_none(),
            "OFF: no other recording is named"
        );
    }

    #[tokio::test]
    async fn propagation_ask_returns_candidates_but_applies_nothing() {
        // The default-policy guarantee: under Ask, gathering candidates must NOT
        // modify any other recording.
        let (db, ada, rec_match, rec_named, _far) = propagation_fixture().await;

        // Snapshot every other recording's name state before.
        let before_match = display_name(&db, &rec_match, 1).await;
        let before_named1 = display_name(&db, &rec_named, 1).await;
        let before_named2 = display_name(&db, &rec_named, 2).await;

        let cands = db
            .propagation_candidates(&ada, 0.5, crate::voiceprint::ScoreNorm::Off)
            .await
            .unwrap();
        assert_eq!(cands.len(), 2, "Ask surfaces candidates");

        // Nothing changed: no display names written, no extra enrollments.
        assert_eq!(display_name(&db, &rec_match, 1).await, before_match);
        assert_eq!(display_name(&db, &rec_named, 1).await, before_named1);
        assert_eq!(display_name(&db, &rec_named, 2).await, before_named2);
        assert!(
            db.named_voice_for(rec_match.as_str(), 1)
                .await
                .unwrap()
                .is_none(),
            "Ask enrolls nothing in the candidate recordings"
        );
        // Ada's sample count is unchanged (only the source sample).
        assert_eq!(named_voice_samples(&db, &ada).await, 1);
    }

    #[tokio::test]
    async fn propagation_auto_backfills_unnamed_keeps_named_and_is_idempotent() {
        let (db, ada, rec_match, rec_named, far) = propagation_fixture().await;
        let cands = db
            .propagation_candidates(&ada, 0.5, crate::voiceprint::ScoreNorm::Off)
            .await
            .unwrap();
        let targets: Vec<(RecordingId, i64)> = cands
            .iter()
            .map(|c| (c.recording_id.clone(), c.speaker_label))
            .collect();

        let applied = db.apply_propagation(&ada, &targets).await.unwrap();
        assert_eq!(applied, 2, "both unnamed Ada-matches back-filled");

        // The unnamed speakers now read "Ada" and are enrolled under it.
        assert_eq!(
            display_name(&db, &rec_match, 1).await.as_deref(),
            Some("Ada")
        );
        assert_eq!(
            display_name(&db, &rec_named, 1).await.as_deref(),
            Some("Ada")
        );
        assert_eq!(
            db.named_voice_for(rec_match.as_str(), 1)
                .await
                .unwrap()
                .as_deref(),
            Some(ada.as_str())
        );
        // The already-named "Bob" is untouched.
        assert_eq!(
            display_name(&db, &rec_named, 2).await.as_deref(),
            Some("Bob")
        );
        // The dissimilar "far" voice is never named.
        assert!(display_name(&db, &far, 1).await.is_none());

        // Idempotent: re-running with the same targets back-fills nothing new.
        let again = db.apply_propagation(&ada, &targets).await.unwrap();
        assert_eq!(again, 0, "re-running propagation does no duplicate work");
        // And re-scanning yields no candidates (they're all named now).
        let after = db
            .propagation_candidates(&ada, 0.5, crate::voiceprint::ScoreNorm::Off)
            .await
            .unwrap();
        assert!(after.is_empty(), "no unnamed matches remain");
    }

    #[tokio::test]
    async fn apply_propagation_never_overwrites_an_existing_name() {
        // Even if a caller hands apply_propagation an already-named target, the
        // name is preserved (defense in depth on top of the candidate filter).
        let (db, ada, _rec_match, rec_named, _far) = propagation_fixture().await;
        let applied = db
            .apply_propagation(&ada, &[(rec_named.clone(), 2)]) // label 2 == "Bob"
            .await
            .unwrap();
        assert_eq!(applied, 0, "an already-named speaker is skipped");
        assert_eq!(
            display_name(&db, &rec_named, 2).await.as_deref(),
            Some("Bob")
        );
    }

    #[tokio::test]
    async fn apply_propagation_skips_target_without_voiceprint() {
        // A target that has a name written but no captured voiceprint must not be
        // left half-applied (display-named but unenrolled): apply rolls the name
        // back so the state is clean.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let src = embedded_recording(None);
        db.insert(&src).await.unwrap();
        db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        let ada = db
            .enroll_speaker(src.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();

        // A recording with NO voiceprint for label 1.
        let novp = embedded_recording(None);
        db.insert(&novp).await.unwrap();

        let applied = db
            .apply_propagation(&ada, &[(novp.id.clone(), 1)])
            .await
            .unwrap();
        assert_eq!(applied, 0, "no voiceprint → nothing enrolled");
        assert!(
            display_name(&db, &novp.id, 1).await.is_none(),
            "the name is rolled back so the target isn't half-applied"
        );
    }

    #[tokio::test]
    async fn propagation_candidates_respect_threshold_strictly() {
        // A voice that's similar but below the bar is not a candidate; raising the
        // bar drops a marginal match.
        let (db, ada, rec_match, _rec_named, _far) = propagation_fixture().await;
        // rec_match's speaker is cos≈0.9999 to Ada — clears 0.5 but let's add a
        // marginal one at ~0.6 and bar it out at 0.8.
        let marg = embedded_recording(None);
        db.insert(&marg).await.unwrap();
        // cos([0.6,0.8,0],[1,0,0]) = 0.6.
        db.save_speaker_voiceprint(marg.id.as_str(), 1, &[0.6, 0.8, 0.0], 0)
            .await
            .unwrap();

        let strict = db
            .propagation_candidates(&ada, 0.8, crate::voiceprint::ScoreNorm::Off)
            .await
            .unwrap();
        let ids: Vec<&str> = strict.iter().map(|c| c.recording_id.as_str()).collect();
        assert!(
            ids.contains(&rec_match.as_str()),
            "strong match survives 0.8"
        );
        assert!(
            !ids.contains(&marg.id.as_str()),
            "0.6 match is below 0.8 bar"
        );
    }

    // ---- Reversible forget (soft-delete) (V5) -------------------------------

    #[tokio::test]
    async fn forget_soft_deletes_then_undo_restores() {
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let src = embedded_recording(None);
        db.insert(&src).await.unwrap();
        db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 5000)
            .await
            .unwrap();
        let ada = db
            .enroll_speaker(src.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(db.list_named_voices().await.unwrap().len(), 1);

        // Forget hides it from listing and recognition.
        assert!(
            db.forget_named_voice(&ada).await.unwrap(),
            "live voice forgotten"
        );
        assert!(
            db.list_named_voices().await.unwrap().is_empty(),
            "forgotten voice is hidden from list-named-voices"
        );
        // A deleted voice doesn't match in recognition.
        let sugg = db
            .recognize_speakers_for(src.id.as_str(), 0.5, crate::voiceprint::ScoreNorm::Off)
            .await
            .unwrap();
        assert!(sugg.is_empty(), "a forgotten voice never matches");
        // Its capture is unlinked.
        assert!(db
            .named_voice_for(src.id.as_str(), 1)
            .await
            .unwrap()
            .is_none());

        // Idempotent: forgetting again is a no-op.
        assert!(
            !db.forget_named_voice(&ada).await.unwrap(),
            "already forgotten"
        );

        // Undo restores it: visible again, capture re-linked, centroid recomputed.
        assert!(
            db.undo_forget(&ada).await.unwrap(),
            "forgotten voice restored"
        );
        let voices = db.list_named_voices().await.unwrap();
        assert_eq!(voices.len(), 1, "restored voice is listed again");
        assert_eq!(voices[0].id, ada);
        assert_eq!(
            db.named_voice_for(src.id.as_str(), 1)
                .await
                .unwrap()
                .as_deref(),
            Some(ada.as_str()),
            "the capture is re-linked on undo"
        );
        assert_eq!(
            named_voice_samples(&db, &ada).await,
            1,
            "centroid recomputed"
        );

        // Idempotent: undo on a live voice is a no-op.
        assert!(!db.undo_forget(&ada).await.unwrap(), "already live");
    }

    #[tokio::test]
    async fn undo_forget_does_not_clobber_a_later_reenrollment() {
        // A capture re-named onto a DIFFERENT voice after the forget keeps its
        // newer assignment when the original is undone.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let src = embedded_recording(None);
        db.insert(&src).await.unwrap();
        db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        let ada = db
            .enroll_speaker(src.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();

        db.forget_named_voice(&ada).await.unwrap();
        // Re-name that speaker to Bob while Ada is forgotten.
        db.set_speaker_name(&src.id, 1, "Bob").await.unwrap();
        let bob = db
            .enroll_speaker(src.id.as_str(), 1, "Bob")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(bob, ada);

        // Undo Ada — must NOT steal the capture back from Bob.
        assert!(db.undo_forget(&ada).await.unwrap());
        assert_eq!(
            db.named_voice_for(src.id.as_str(), 1)
                .await
                .unwrap()
                .as_deref(),
            Some(bob.as_str()),
            "a re-enrolled capture keeps its newer voice"
        );
        assert_eq!(
            named_voice_samples(&db, &ada).await,
            0,
            "Ada has no captures back"
        );
    }

    #[tokio::test]
    async fn reusing_a_forgotten_name_makes_a_fresh_voice() {
        // Naming a new speaker with a forgotten voice's name creates a new live
        // voice, not a revival of the tombstone.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let src = embedded_recording(None);
        db.insert(&src).await.unwrap();
        db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        let ada1 = db
            .enroll_speaker(src.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();
        db.forget_named_voice(&ada1).await.unwrap();

        let rec = embedded_recording(None);
        db.insert(&rec).await.unwrap();
        db.save_speaker_voiceprint(rec.id.as_str(), 1, &[0.0, 1.0, 0.0], 0)
            .await
            .unwrap();
        let ada2 = db
            .enroll_speaker(rec.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(ada1, ada2, "a forgotten name yields a fresh voice id");
        let voices = db.list_named_voices().await.unwrap();
        assert_eq!(voices.len(), 1, "only the new live Ada is listed");
        assert_eq!(voices[0].id, ada2);
    }

    #[tokio::test]
    async fn name_propagation_config_defaults_to_ask() {
        // The DEFAULT-Ask contract at the config layer.
        let diar = crate::config::DiarizationConfig::default();
        assert_eq!(diar.name_propagation, crate::config::NamePropagation::Ask);
        // Round-trips through serde as snake_case.
        let json = serde_json::to_string(&crate::config::NamePropagation::Auto).unwrap();
        assert_eq!(json, "\"auto\"");
        let off: crate::config::NamePropagation = serde_json::from_str("\"off\"").unwrap();
        assert_eq!(off, crate::config::NamePropagation::Off);
        // A config missing the field deserializes to Ask (serde default).
        let de: crate::config::NamePropagation = serde_json::from_str("null").unwrap_or_default();
        assert_eq!(de, crate::config::NamePropagation::Ask);
    }

    #[tokio::test]
    async fn soft_delete_migration_applies_and_deleted_at_filters() {
        // The migration chain applies cleanly on a fresh DB (Catalog::open runs
        // it) and the new column exists + filters listing. A direct UPDATE proves
        // the column is queryable and the read path honors it.
        let db = Catalog::open(Path::new("sqlite::memory:")).await.unwrap();
        let src = embedded_recording(None);
        db.insert(&src).await.unwrap();
        db.save_speaker_voiceprint(src.id.as_str(), 1, &[1.0, 0.0, 0.0], 0)
            .await
            .unwrap();
        let ada = db
            .enroll_speaker(src.id.as_str(), 1, "Ada")
            .await
            .unwrap()
            .unwrap();
        // Stamp deleted_at directly (the column exists post-migration).
        sqlx::query("UPDATE named_voiceprints SET deleted_at = datetime('now') WHERE id = ?")
            .bind(&ada)
            .execute(&db.pool)
            .await
            .unwrap();
        assert!(
            db.list_named_voices().await.unwrap().is_empty(),
            "deleted_at IS NOT NULL is filtered from listing"
        );
    }
}
