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
//!   `parse_status`/`as_str`. A status the parser doesn't recognize errors the
//!   whole query, so every variant needs an arm.
//! - **Machine truth vs. user edits.** `original_transcript` (raw ASR) and
//!   `clean_transcript` (pipeline output) are kept so a hand edit to the live
//!   `transcript` stays reversible. Segments live in their own tables and are
//!   replaced wholesale on every (re)transcribe; user edits never rewrite them.
//! - **Search is hybrid.** Lexical FTS5 and per-chunk vector cosine are computed
//!   separately, fused with RRF ([`crate::fusion`]), and de-duplicated on a
//!   meeting-stable key so a meeting's two tracks collapse to one result.
//! - **WAL with bounded growth.** The pool runs in WAL mode; [`Catalog::open`]
//!   caps the WAL size and the daemon calls [`Catalog::checkpoint`] on idle so a
//!   long-lived reader can't let it grow without bound.

use crate::config::AnnConfig;
use crate::error::Result;
use crate::id::RecordingId;
use crate::tags::Tag;
use crate::types::{
    AiActivityEntry, DictationHistoryEntry, Entity, EntityFacet, ListFilter, MeetingDigest,
    NamedVoice, PropagationCandidate, Recording, RecordingStatus, SavedSearch, SpeakerName,
    SpeakerSuggestion, TranscriptSegment, TranscriptWord,
};
use chrono::{DateTime, Local};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::path::{Path, PathBuf};
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
    /// The deserialized, L2-normalized embedding. `None` marks a blob that failed
    /// the 4-byte-alignment guard at load time, so the ranking paths skip it
    /// without re-reading SQLite — the same warn-and-skip a direct decode does.
    vector: Option<Vec<f32>>,
}

/// The deserialized embedding corpus, cached in memory so a search doesn't
/// re-read and re-decode every blob from SQLite on every query.
///
/// Two flat lists mirror the two SQL queries the ranking paths run on each
/// search: the per-chunk vectors (the primary, high-recall path) and the legacy
/// whole-recording vectors (the backfill fallback). The ranking code keeps its
/// own dimension/precedence logic; the corpus only removes the disk read +
/// decode, it never changes which vectors are considered.
#[derive(Debug, Clone, Default)]
pub(crate) struct EmbeddingCorpus {
    /// Every row of `embedding_chunks`, decoded once. Each vector sits behind an
    /// `Arc` so a single-recording update can copy-on-write a fresh corpus that
    /// shares every other recording's vectors by pointer — no deep copy, no
    /// SQLite re-decode (see `patch_recording_in_cache`).
    chunks: Vec<Arc<CachedVector>>,
    /// Every row of `embeddings` (legacy whole-recording), decoded once. Held
    /// behind an `Arc` for the same cheap copy-on-write as `chunks`.
    legacy: Vec<Arc<CachedVector>>,
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
/// vectors live as little-endian f32 blobs in `embedding_chunks` / `embeddings`.
/// A naive implementation `SELECT`s and decodes the entire corpus from disk on
/// every query, and that cost grows with the library and dominates a typed query
/// or a RAG turn.
///
/// `embedding_cache` holds the decoded corpus in memory so repeated queries
/// reuse it. The design is deliberately simple and pessimistic about staleness:
///
/// - **One whole-corpus snapshot** whose vectors are each `Arc`-held, built lazily
///   on the first query. The ranking loops iterate the full corpus, so a flat
///   snapshot mirrors them; holding each vector behind an `Arc` is what lets a
///   single-recording change copy-on-write a fresh snapshot cheaply (below).
/// - **Incremental single-recording updates.** A single embed (`upsert_embedding`,
///   `upsert_chunk_embeddings`) or a recording `delete` calls
///   `patch_recording_in_cache`: it re-reads only that recording's rows and swaps
///   them into a fresh snapshot that shares every other recording's vectors by
///   `Arc` pointer. So recording one new memo doesn't force the next search to
///   re-decode the entire library from SQLite. Bulk wipes (`clear_all_embeddings`,
///   `clear_all_recordings`, the retention sweep) still drop the snapshot
///   wholesale and let the next query rebuild, and any targeted reload that errors
///   falls back to that same coarse drop — a stale vector that ranks wrongly is
///   never an acceptable outcome.
/// - **Bounded.** A corpus over a fixed vector-count cap is left uncached; those
///   (rare, very large) libraries fall back to reading from SQLite each query, so
///   memory stays bounded regardless of archive size.
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
    /// Monotonic generation, bumped on every embedding-cache invalidation. A
    /// corpus rebuild snapshots this before its SQL reads and, under the cache
    /// write lock at store time, only caches the snapshot when the generation is
    /// unchanged — so an invalidation that races the rebuild can't be lost. See
    /// the "Lost-invalidation safe" note above.
    embedding_cache_gen: Arc<AtomicU64>,
    /// The optional approximate-nearest-neighbour index for semantic search, or
    /// `None` when it's disabled, cold, or unhealthy — in which case retrieval
    /// uses the brute-force cosine scan over `embedding_cache`. Mirrors
    /// `embedding_cache` exactly (`Arc<RwLock<Option<…>>>`, shared across the
    /// daemon's clones). Always `None` unless the `ann-usearch` feature is
    /// compiled *and* `ann_config.enabled` is set *and* the daemon background-
    /// built it; `Catalog::open` never builds it, so startup never blocks. See
    /// [`crate::catalog::ann`].
    ann: Arc<RwLock<Option<ann::AnnIndex>>>,
    /// The ANN tuning config, set by the daemon via [`Catalog::set_ann_config`]
    /// after `open` when the feature is enabled. Defaults to
    /// [`AnnConfig::default`] (disabled), so a catalog opened without that call
    /// — every existing caller, every test — keeps the brute-force behaviour
    /// unchanged.
    ann_config: Arc<RwLock<AnnConfig>>,
    /// Where the ANN index persists, derived from the catalog path in `open`
    /// (`catalog.db` → `catalog.ann`). `None` for an in-memory catalog
    /// (`sqlite::memory:`), which has no on-disk home for a sidecar.
    ann_sidecar: Option<PathBuf>,
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

/// How many recent in-place dictations to keep in the re-grab ring buffer.
/// A short convenience history, not an archive — every insert prunes past this.
const DICTATION_HISTORY_KEEP: i64 = 50;

/// Per-row char cap on a stored dictation's `text`. Row count is already bounded
/// by `DICTATION_HISTORY_KEEP`, but a single pathologically long dictation
/// shouldn't bloat a row; this ceiling sits far above any normal dictation, so
/// only an extreme outlier is truncated (with a marker). Mirrors
/// `AI_ACTIVITY_FIELD_MAX_CHARS`.
const DICTATION_HISTORY_TEXT_MAX_CHARS: usize = 64 * 1024;

/// Turn a user's search box text into a safe FTS5 MATCH expression.
///
/// Each term is wrapped in a double-quoted FTS5 string rather than stripped down
/// to bare alphanumerics, so punctuation inside a term (`react-router`,
/// `O'Connor`, a code snippet) is handed to FTS5's tokenizer instead of being
/// thrown away. Embedded double-quotes are escaped by doubling them, so the input
/// can never break out of the quoting (no MATCH syntax error, no injection).
///
/// - A run of non-whitespace is one term, matched as a prefix (`"term"*`).
/// - A `"quoted span"` is kept together as an exact phrase (no prefix), so a user
///   who quotes "fix the bug" gets that phrase, not three prefix terms.
/// - Terms are AND-ed: every one must match. An empty/whitespace query yields an
///   empty string (callers treat that as "no query").
fn sanitize_fts5_query(query: &str) -> String {
    let mut terms: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut in_quote = false;

    // Emit `buf` as one quoted FTS5 term (prefix unless it was an explicit
    // phrase), escaping embedded quotes. Blank buffers are dropped.
    fn flush(buf: &mut String, terms: &mut Vec<String>, prefix: bool) {
        let t = buf.trim();
        if !t.is_empty() {
            let escaped = t.replace('"', "\"\"");
            terms.push(if prefix {
                format!("\"{escaped}\"*")
            } else {
                format!("\"{escaped}\"")
            });
        }
        buf.clear();
    }

    for c in query.chars() {
        match c {
            '"' if in_quote => {
                in_quote = false;
                flush(&mut buf, &mut terms, false); // closed phrase → exact
            }
            '"' => {
                flush(&mut buf, &mut terms, true); // flush the pending bare term
                in_quote = true;
            }
            c if c.is_whitespace() && !in_quote => flush(&mut buf, &mut terms, true),
            c => buf.push(c),
        }
    }
    // Trailing bare term, or an unterminated quote (treated as a plain phrase —
    // its `OR`/`AND`/`*` are literal tokens inside quotes, so still injection-safe).
    flush(&mut buf, &mut terms, !in_quote);

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

/// The result of a [`Catalog::find_replace_transcript_library`] — the
/// across-all-recordings find-and-replace. Aggregates how many recordings were
/// actually rewritten and the grand total of occurrences replaced, plus the
/// per-recording `(id, new transcript)` for each recording that changed.
///
/// Only recordings with at least one match appear in `changed`: a zero-match
/// recording is skipped entirely (no write, no version churn, no event), so the
/// caller can run its per-recording re-flow/re-embed/event upkeep over exactly
/// the recordings that were touched.
///
/// A recording whose update *errored* (anything other than the benign
/// no-transcript `NotFound`, which is a normal skip) is counted in `failed` and
/// its id pushed to `failed_ids`, so the caller can tell the user that some rows
/// errored rather than silently reporting a smaller success count.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FindReplaceLibraryOutcome {
    /// Number of recordings whose live transcript was actually rewritten.
    pub recordings_changed: usize,
    /// Grand total of occurrences replaced across every changed recording.
    pub total_replacements: usize,
    /// `(id, new transcript)` for each recording that changed, so the caller can
    /// re-flow timing, re-embed, and emit `transcript_updated` per recording.
    pub changed: Vec<(RecordingId, String)>,
    /// Number of recordings whose update errored (excluding the benign
    /// no-transcript skip), so a partial failure isn't hidden as a smaller
    /// success count.
    pub failed: usize,
    /// The ids of the recordings counted in `failed`, for diagnostics.
    pub failed_ids: Vec<RecordingId>,
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

/// One retrieved evidence chunk for Ask-my-archive: enough to ground a prompt
/// and to map an answer citation back to its source recording + chunk.
///
/// Produced by [`Catalog::retrieve_context`], which rides the same hybrid
/// (vector + FTS5 + RRF) retrieval the search bar uses but recovers the single
/// best-matching chunk per result so a citation can point at the exact passage.
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievedChunk {
    /// The representative recording this evidence came from (meeting-deduped).
    pub recording_id: RecordingId,
    /// The recording's `meeting_id`, if it is one track of a meeting.
    pub meeting_id: Option<String>,
    /// 0-based chunk index from [`crate::chunk::chunk_transcript`], or `-1` for a
    /// lexical-only / legacy-only hit that has no per-chunk vector to argmax over.
    pub chunk_index: i64,
    /// The chunk's transcript text, re-derived from the live transcript via
    /// `chunk_transcript` (or a transcript prefix for a lexical/legacy hit).
    /// Never empty — a citation with no snippet can't ground anything.
    pub text: String,
    /// Calibrated relevance of the best-matching chunk to the query, in 0..1
    /// ([`crate::fusion::calibrate_cosine`] of the best cosine).
    pub relevance: f32,
    /// `true` when this is a lexical (FTS5) / legacy hit surfaced without a usable
    /// per-chunk vector — drives the snippet fallback and the relevance floor.
    pub is_lexical: bool,
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
/// Matching is done by regex rather than by slicing a lowercased copy back onto
/// the original. `char::to_lowercase` can change a string's length (some
/// locale-specific folds), so lowercasing the haystack and reusing those byte
/// offsets against the original would be unsound; the regex matches on the
/// original directly, keeping every byte offset valid and the unmatched text
/// byte-for-byte intact.
fn replace_ignore_case(haystack: &str, needle: &str, replacement: &str) -> (usize, String) {
    // An empty needle would match at every position — nothing to replace.
    if needle.is_empty() {
        return (0, haystack.to_string());
    }
    // Literal, case-insensitive search: escape the needle so its regex
    // metacharacters match verbatim, and `(?i)` gives Unicode-aware case folding
    // (so "café" matches "CAFÉ"). `regex::escape` can't produce an invalid
    // pattern, so the compile only fails on a pathological size limit — treat that
    // as "no match" rather than panicking on user input.
    let re = match regex::Regex::new(&format!("(?i){}", regex::escape(needle))) {
        Ok(re) => re,
        Err(_) => return (0, haystack.to_string()),
    };
    // One pass: the closure counts matches and returns the literal replacement. A
    // closure replacer doesn't expand `$1`/`$name`, so the replacement goes in
    // verbatim, and replace_all never re-scans inserted text.
    let mut count = 0usize;
    let out = re
        .replace_all(haystack, |_: &regex::Captures<'_>| {
            count += 1;
            replacement.to_string()
        })
        .into_owned();
    (count, out)
}

pub mod ann;
mod chapters;
mod embeddings;
pub use embeddings::AnnHealth;
mod entities;
mod meeting_digests;
mod recordings;
mod saved_search;
mod segments;
mod speakers;
mod tags;

#[cfg(test)]
mod tests;

impl Catalog {
    /// Open (or create) a catalog database at `path`. Runs pending migrations.
    ///
    /// WAL configuration notes:
    /// - `journal_mode=WAL` + `synchronous=NORMAL` → ACID with crash safety,
    ///   no fsync per write.
    /// - `wal_autocheckpoint=1000` triggers an automatic checkpoint when the
    ///   WAL reaches ~1000 pages (~4 MB), which bounds WAL growth day-to-day.
    ///   Long-lived readers can still defer a checkpoint, so
    ///   [`Catalog::checkpoint`] is available to force one on demand.
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
            // locked") the instant a connection can't take the lock. With a small
            // pool (max_connections below) a reader and the daemon's writer can
            // briefly contend, so wait up to 5s for the lock rather than failing
            // the whole query immediately.
            .busy_timeout(std::time::Duration::from_secs(5))
            .pragma("wal_autocheckpoint", "1000")
            .pragma("journal_size_limit", "67108864");

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;
        // Derive the ANN sidecar next to the database (catalog.db → catalog.ann).
        // An in-memory database (`sqlite::memory:`, used by tests) has no on-disk
        // file, so it gets no sidecar — the ANN then runs index-in-memory only,
        // never persisting, which is exactly what the tests want.
        let ann_sidecar = if path.is_absolute() || path.extension().is_some() {
            path.parent().map(|dir| {
                let stem = path
                    .file_stem()
                    .map(|s| s.to_os_string())
                    .unwrap_or_else(|| std::ffi::OsString::from("catalog"));
                let mut name = stem;
                name.push(".ann");
                dir.join(name)
            })
        } else {
            None
        };
        Ok(Self {
            pool,
            embedding_cache: Arc::new(RwLock::new(None)),
            embedding_cache_gen: Arc::new(AtomicU64::new(0)),
            ann: Arc::new(RwLock::new(None)),
            ann_config: Arc::new(RwLock::new(AnnConfig::default())),
            ann_sidecar,
        })
    }
}

/// Decode one `(id, vector, meeting_id)` embedding row into a [`CachedVector`].
///
/// The vector is stored as little-endian f32 bytes; a blob whose length isn't a
/// multiple of 4 is kept as `vector: None` (and warned) so the ranking paths skip
/// it. The cache must not silently resurrect a corrupt blob as a zero-length
/// vector.
fn row_to_cached_vector(row: &sqlx::sqlite::SqliteRow) -> Result<CachedVector> {
    let id: String = row.try_get("id")?;
    let meeting_id: Option<String> = row.try_get("meeting_id")?;
    let bytes: Vec<u8> = row.try_get("vector")?;
    let vector = if bytes.len().is_multiple_of(4) {
        Some(
            bytes
                .chunks_exact(4)
                .map(|c| {
                    f32::from_le_bytes(
                        c.try_into()
                            .expect("chunks_exact(4) yields exactly 4 bytes"),
                    )
                })
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
        pinned: row.try_get("pinned").unwrap_or(false),
        tag_suggestions: row
            .try_get::<Option<String>, _>("tag_suggestions")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        summary: row.try_get("summary").unwrap_or(None),
        summary_model: row.try_get("summary_model").unwrap_or(None),
        // The entity-extraction model (nullable). `unwrap_or(None)` keeps older
        // rows that predate the column NULL.
        entities_model: row.try_get("entities_model").unwrap_or(None),
        // The auto-chapter model (nullable). `unwrap_or(None)` keeps older rows
        // that predate the column NULL.
        chapters_model: row.try_get("chapters_model").unwrap_or(None),
        title: row.try_get("title").unwrap_or(None),
        title_is_auto: row.try_get("title_is_auto").unwrap_or(true),
        title_model: row.try_get("title_model").unwrap_or(None),
        tag_model: row.try_get("tag_model").unwrap_or(None),
        diarization_model: row.try_get("diarization_model").unwrap_or(None),
        // Mean per-word ASR confidence (nullable). `unwrap_or(None)` keeps older
        // rows that predate the column NULL — no badge, never flagged.
        mean_confidence: row.try_get("mean_confidence").unwrap_or(None),
        // Detected spoken language (nullable). `unwrap_or(None)` keeps older rows
        // that predate the column NULL — no badge, never routed.
        detected_language: row.try_get("detected_language").unwrap_or(None),
        tags: Vec::new(),
        // Populated separately (child query against `entities`) by list/get, like `tags`.
        entities: Vec::new(),
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
