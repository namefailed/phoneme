use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::atomic::{AtomicU16, Ordering};

/// A recording identifier: `YYYYMMDDTHHmmssMMM` (18 chars: 8 date + 1 `T` +
/// 6 time + 3 milliseconds). Sortable as a plain string; uniqueness within
/// a process is guaranteed by an atomic per-millisecond sequence bump.
///
/// Example: `20260519T143500823`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RecordingId(String);

static LAST_TS_MS: AtomicU16 = AtomicU16::new(0);

impl RecordingId {
    /// Generate a new id from the current local time.
    ///
    /// If called more than once within the same millisecond, the second call
    /// will bump the millisecond field by one to preserve uniqueness without
    /// blocking the caller.
    pub fn new() -> Self {
        Self::from_datetime(Local::now())
    }

    /// Generate an id from a specific datetime (used by tests).
    pub fn from_datetime(dt: DateTime<Local>) -> Self {
        let mut ms = dt.timestamp_subsec_millis() as u16;
        let prev = LAST_TS_MS.swap(ms, Ordering::SeqCst);
        if prev == ms {
            ms = ms.wrapping_add(1);
            LAST_TS_MS.store(ms, Ordering::SeqCst);
        }
        let s = format!("{}{:03}", dt.format("%Y%m%dT%H%M%S"), ms);
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
    use std::sync::Mutex;

    /// `from_datetime` mutates the module-global `LAST_TS_MS`. Cargo runs
    /// tests in this module in parallel by default, which races on that
    /// state and can make `ids_are_unique_within_same_millisecond` flake.
    /// Every test in this module acquires this lock to serialize against
    /// each other. Tests in other modules don't touch `LAST_TS_MS`, so this
    /// is sufficient.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn id_has_expected_shape() {
        let _g = TEST_LOCK.lock().unwrap();
        let dt = Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap();
        let id = RecordingId::from_datetime(dt);
        // 18 chars: YYYYMMDDTHHmmssMMM (3-digit milliseconds)
        assert_eq!(id.as_str().len(), 18);
        assert!(id.as_str().starts_with("20260519T143500"));
    }

    #[test]
    fn file_stem_drops_date_prefix() {
        let _g = TEST_LOCK.lock().unwrap();
        let dt = Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap();
        let id = RecordingId::from_datetime(dt);
        assert_eq!(id.file_stem().len(), 9);
        assert!(id.file_stem().starts_with("143500"));
    }

    #[test]
    fn day_folder_format() {
        let _g = TEST_LOCK.lock().unwrap();
        let dt = Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap();
        let id = RecordingId::from_datetime(dt);
        assert_eq!(id.day_folder(), "2026-05-19");
    }

    #[test]
    fn ids_are_unique_within_same_millisecond() {
        let _g = TEST_LOCK.lock().unwrap();
        // Reset so the assertion only depends on this test's two calls.
        LAST_TS_MS.store(0, Ordering::SeqCst);
        let dt = Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap();
        let a = RecordingId::from_datetime(dt);
        let b = RecordingId::from_datetime(dt);
        assert_ne!(a, b);
    }

    #[test]
    fn ids_sort_chronologically() {
        let _g = TEST_LOCK.lock().unwrap();
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
