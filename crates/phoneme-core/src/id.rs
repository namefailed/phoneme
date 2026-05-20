use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Mutex;

/// A recording identifier: `YYYYMMDDTHHmmssMMM` (18 chars: 8 date + 1 `T` +
/// 6 time + 3 milliseconds). Sortable as a plain string; uniqueness within
/// a process is guaranteed by a global state that bumps the millisecond field
/// on collisions.
///
/// Example: `20260519T143500823`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RecordingId(String);

/// Tracks the last-seen second + the last millisecond value we emitted in
/// that second. Wrapped in a Mutex so concurrent `from_datetime` calls are
/// serialized — the previous AtomicU16-based algorithm raced when multiple
/// threads interleaved their swap/store operations, occasionally producing
/// duplicate IDs (caught by Task 7's catalog tests under parallel load).
struct IdState {
    last_second_key: i64,
    last_used_ms: u16,
}

static STATE: Mutex<IdState> = Mutex::new(IdState {
    last_second_key: 0,
    last_used_ms: 0,
});

impl RecordingId {
    /// Generate a new id from the current local time.
    ///
    /// If called more than once within the same wall-clock second, subsequent
    /// calls bump the millisecond field by one from the previous emission to
    /// guarantee process-wide uniqueness without blocking on the wall clock.
    pub fn new() -> Self {
        Self::from_datetime(Local::now())
    }

    /// Generate an id from a specific datetime.
    pub fn from_datetime(dt: DateTime<Local>) -> Self {
        let second_key = dt.timestamp();
        let mut state = STATE.lock().expect("RecordingId state mutex poisoned");
        let ms = if state.last_second_key == second_key {
            // Same second as a prior call — bump from the previous emitted ms
            // so this id is strictly greater than the last one in this second.
            state.last_used_ms.wrapping_add(1)
        } else {
            // Different second (or first call ever) — start fresh from the
            // datetime's actual subsecond field.
            state.last_second_key = second_key;
            dt.timestamp_subsec_millis() as u16
        };
        state.last_used_ms = ms;
        drop(state);
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
