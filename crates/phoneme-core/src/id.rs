use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Mutex;

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

static COUNTER: Mutex<u64> = Mutex::new(0);

impl RecordingId {
    /// Generate a new id from the current local time.
    pub fn new() -> Self {
        Self::from_datetime(Local::now())
    }

    /// Generate an id from a specific datetime.
    pub fn from_datetime(dt: DateTime<Local>) -> Self {
        let mut counter = COUNTER.lock().expect("RecordingId counter mutex poisoned");
        *counter = counter.wrapping_add(1);
        let suffix = (*counter % 1000) as u16;
        drop(counter);
        let s = format!("{}{:03}", dt.format("%Y%m%dT%H%M%S"), suffix);
        Self(s)
    }

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
    pub fn from_string(s: String) -> Self {
        Self(s)
    }
}

impl RecordingId {
    /// Construct from a known-valid id string (e.g., from DB rows). Does not validate.
    pub(crate) fn from_str_unchecked(s: &str) -> Self {
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

    // No TEST_LOCK or atomic reset needed any more: `from_datetime` now
    // serializes all callers through the module-global Mutex, so tests can
    // run in parallel and still get distinct ids.

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
