//! The recording identifier.
//!
//! This module owns [`RecordingId`], the timestamp-shaped string that names
//! every recording. The daemon mints one the moment a recording starts, and it
//! threads through everything afterwards — the catalog primary key, the WAV's
//! path on disk (the id's date prefix *is* the day folder and file stem), inbox
//! payload filenames, and IPC.
//!
//! Two invariants make that work: the id sorts chronologically (so a plain
//! string sort orders recordings by time), and it is unique within a process
//! even when two are generated in the same wall-clock millisecond (a monotonic
//! counter in the last three digits). The fixed-offset accessors
//! ([`RecordingId::file_stem`], [`RecordingId::day_folder`]) slice at byte
//! positions, so anything reconstructing an id from outside the process should
//! go through [`RecordingId::parse`] to avoid a panic on a malformed string.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// A recording identifier: `YYYYMMDDTHHmmssNNN` (18 chars: 8 date + 1 `T` +
/// 6 time + 3-digit per-process monotonic counter, mod 1000).
///
/// The trailing 3 digits are NOT the actual subsecond field — they're a
/// monotonic counter that disambiguates IDs generated within the same
/// wall-clock second. The wall-clock prefix gives chronological ordering;
/// the counter suffix guarantees process-wide uniqueness.
///
/// Why not bump-on-collision with real ms? An earlier version tracked
/// `(last_second_key, last_used_ms)` and bumped only on same-second hits.
/// That raced: if another thread's call between our two calls had a different
/// `second_key`, the RESET branch in that thread replaced `last_second_key`,
/// then our next call ALSO went through RESET and reused `dt.subsec_millis`
/// — producing a duplicate of the first ID. A pure monotonic counter
/// sidesteps the entire race.
///
/// Example: `20260519T143500042`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RecordingId(String);

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl RecordingId {
    /// Generate a new id from the current local time.
    pub fn new() -> Self {
        Self::from_datetime(Local::now())
    }

    /// Generate an id from a specific datetime.
    pub fn from_datetime(dt: DateTime<Local>) -> Self {
        let current = COUNTER.fetch_add(1, Ordering::Relaxed);
        let suffix = (current % 1000) as u16;
        let s = format!("{}{:03}", dt.format("%Y%m%dT%H%M%S"), suffix);
        Self(s)
    }

    /// The id in its canonical 18-char string form.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Time portion as `HHmmssMMM` — the WAV filename within a day folder.
    pub fn file_stem(&self) -> &str {
        &self.0[9..] // skip `YYYYMMDDT`
    }

    /// Day portion as `YYYY-MM-DD` — the day folder name under audio_dir.
    pub fn day_folder(&self) -> String {
        format!("{}-{}-{}", &self.0[0..4], &self.0[4..6], &self.0[6..8])
    }

    /// Reconstruct an id from its canonical string form (e.g. when the user
    /// pastes it on the CLI). The format isn't re-validated — RecordingIds
    /// flow through the catalog and IPC layer as opaque strings already.
    ///
    /// Prefer [`RecordingId::parse`] for input that originates outside the
    /// process (CLI args, IPC payloads): an id that fails the length/shape
    /// check would otherwise panic later in `file_stem()` / `day_folder()`,
    /// which slice at fixed byte offsets.
    pub fn from_string(s: String) -> Self {
        Self(s)
    }

    /// Parse and validate a user-supplied id string. Returns `None` unless the
    /// string is the canonical 18-char `YYYYMMDDTHHmmssNNN` shape. Callers
    /// should map `None` to a "not found" error rather than risk a panic in
    /// the fixed-offset slicing accessors.
    ///
    /// ```
    /// use phoneme_core::RecordingId;
    /// let id = RecordingId::parse("20260519T143500042").unwrap();
    /// assert_eq!(id.day_folder(), "2026-05-19");
    /// assert_eq!(id.file_stem(), "143500042");
    /// assert!(RecordingId::parse("not-an-id").is_none());
    /// ```
    pub fn parse(s: impl Into<String>) -> Option<Self> {
        let s = s.into();
        let bytes = s.as_bytes();
        if s.len() == 18
            && bytes[..8].iter().all(u8::is_ascii_digit)
            && bytes[8] == b'T'
            && bytes[9..].iter().all(u8::is_ascii_digit)
        {
            Some(Self(s))
        } else {
            None
        }
    }
}

impl RecordingId {
    /// Construct from a known-valid id string (e.g., from DB rows). Does not
    /// validate in release builds; the `debug_assert` catches malformed ids in
    /// tests and debug builds so corruption surfaces early instead of as a
    /// slice-out-of-bounds panic in `file_stem()` / `day_folder()`.
    pub(crate) fn from_str_unchecked(s: &str) -> Self {
        debug_assert_eq!(
            s.len(),
            18,
            "RecordingId must be 18 chars (YYYYMMDDTHHmmssNNN), got: {s:?}"
        );
        Self(s.to_string())
    }
}

impl fmt::Display for RecordingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Default for RecordingId {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    // No TEST_LOCK or atomic reset needed any more: `from_datetime` bumps a
    // lock-free `AtomicU64` (relaxed fetch_add) for the disambiguating suffix,
    // so concurrent callers always get distinct ids and tests can run in
    // parallel.

    #[test]
    fn id_has_expected_shape() {
        let dt = Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap();
        let id = RecordingId::from_datetime(dt);
        // 18 chars: YYYYMMDDTHHmmssMMM (3-digit milliseconds)
        assert_eq!(id.as_str().len(), 18);
        assert!(id.as_str().starts_with("20260519T143500"));
    }

    #[test]
    fn file_stem_drops_date_prefix() {
        let dt = Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap();
        let id = RecordingId::from_datetime(dt);
        assert_eq!(id.file_stem().len(), 9);
        assert!(id.file_stem().starts_with("143500"));
    }

    #[test]
    fn day_folder_format() {
        let dt = Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap();
        let id = RecordingId::from_datetime(dt);
        assert_eq!(id.day_folder(), "2026-05-19");
    }

    #[test]
    fn ids_are_unique_within_same_millisecond() {
        let dt = Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap();
        let a = RecordingId::from_datetime(dt);
        let b = RecordingId::from_datetime(dt);
        assert_ne!(a, b);
    }

    #[test]
    fn parse_accepts_canonical_form() {
        assert!(RecordingId::parse("20260519T143500042").is_some());
        assert_eq!(
            RecordingId::parse("20260519T143500042").unwrap().as_str(),
            "20260519T143500042"
        );
    }

    #[test]
    fn parse_rejects_malformed_ids() {
        assert!(RecordingId::parse("garbage").is_none()); // too short
        assert!(RecordingId::parse("20260519X143500042").is_none()); // wrong separator
        assert!(RecordingId::parse("20260519T14350004").is_none()); // 17 chars
        assert!(RecordingId::parse("20260519T1435000423").is_none()); // 19 chars
        assert!(RecordingId::parse("2026051AT143500042").is_none()); // non-digit in date
    }

    #[test]
    fn ids_sort_chronologically() {
        let mut ids = [
            RecordingId::from_datetime(Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap()),
            RecordingId::from_datetime(Local.with_ymd_and_hms(2026, 5, 19, 9, 0, 0).unwrap()),
            RecordingId::from_datetime(Local.with_ymd_and_hms(2026, 5, 19, 18, 0, 0).unwrap()),
        ];
        ids.sort();
        assert_eq!(ids[0].as_str()[9..15].to_string(), "090000");
        assert_eq!(ids[1].as_str()[9..15].to_string(), "143500");
        assert_eq!(ids[2].as_str()[9..15].to_string(), "180000");
    }
}
