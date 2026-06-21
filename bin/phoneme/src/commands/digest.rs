//! `phoneme digest` — generate (or view) a period digest: one LLM rollup across
//! every recording in a date window.
//!
//! Generate (the default) rides the spawning path like `phoneme summarize`: the
//! daemon ACKs immediately and rolls up in the background, storing the result and
//! emitting `PeriodDigestUpdated` / `PeriodDigestFailed`. `--show` rides the
//! read-only observe path (a down daemon is the answer, not something to fix by
//! spawning one) and prints the stored digest for the resolved window, or
//! "no digest yet" when none exists for that exact range.
//!
//! The range is one of `--daily` (default: the current calendar day),
//! `--weekly` (the last 7 calendar days), or an explicit `--since/--until`.
//! Boundary semantics: the window is **inclusive on both ends** and snapped to
//! calendar-day boundaries — `--daily` is local-midnight-today through
//! end-of-day-today, `--weekly` is six days ago at midnight through end-of-day-
//! today. Snapping to whole days (rather than "…→ now") makes the derived range
//! *key* stable, so `phoneme digest --show` later in the same day fetches the
//! digest a prior `phoneme digest` stored.

use crate::args::DigestArgs;
use crate::client::Client;
use crate::output;
use chrono::{DateTime, Local};
use phoneme_core::{Config, PeriodDigest};
use phoneme_ipc::Request;
use std::process::ExitCode;

/// The resolved digest window: canonical bounds, the human label, and the stable
/// storage key the daemon derives from the bounds (kept in sync with the daemon's
/// `period_digest_key`). Pure output of [`resolve_range`], so it can be unit-tested
/// without a daemon.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRange {
    /// Lower bound (inclusive).
    pub since: DateTime<Local>,
    /// Upper bound (inclusive).
    pub until: DateTime<Local>,
    /// Human label for the period (stored for display).
    pub label: String,
}

impl ResolvedRange {
    /// The stable range key, identical to the daemon's `period_digest_key`
    /// (`"{since_rfc3339}|{until_rfc3339}"`). Used by `--show` to fetch the
    /// digest the matching generate stored.
    pub fn key(&self) -> String {
        format!("{}|{}", self.since.to_rfc3339(), self.until.to_rfc3339())
    }
}

/// Parse a `--since`/`--until` value: a full RFC 3339 timestamp, or a bare
/// `YYYY-MM-DD` date interpreted at local start-of-day. Mirrors `phoneme list`'s
/// date parsing so both commands accept the same forms.
fn parse_date(s: &str) -> Option<DateTime<Local>> {
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.with_timezone(&Local));
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|naive| {
            use chrono::TimeZone;
            Local.from_local_datetime(&naive).single()
        })
}

/// Local start-of-day (midnight) for the calendar day `d` falls on, anchored to
/// `Local`. Falls back to `d` itself on the (DST/edge) cases where midnight is
/// ambiguous or skipped, so the function is total.
fn start_of_day(d: DateTime<Local>) -> DateTime<Local> {
    use chrono::TimeZone;
    d.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
        .unwrap_or(d)
}

/// Local end-of-day (23:59:59) for the calendar day `d` falls on. Used as the
/// inclusive upper bound so a whole-day window is deterministic (independent of
/// the exact `now`), which keeps the derived range key stable across `digest`
/// and a later `digest --show`.
fn end_of_day(d: DateTime<Local>) -> DateTime<Local> {
    use chrono::TimeZone;
    d.date_naive()
        .and_hms_opt(23, 59, 59)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
        .unwrap_or(d)
}

/// Resolve the CLI range flags into a concrete window + label, anchored at `now`
/// (passed in so the resolution is deterministic in tests). Precedence:
/// explicit `--since/--until` → `--weekly` → `--daily` (the default). Both
/// `--daily` and `--weekly` snap to whole calendar days (midnight → end-of-day)
/// so the derived key is stable within the day. Returns a human error string on
/// a malformed/invalid custom range.
pub fn resolve_range(args: &DigestArgs, now: DateTime<Local>) -> Result<ResolvedRange, String> {
    if let (Some(since_s), Some(until_s)) = (&args.since, &args.until) {
        let since =
            parse_date(since_s).ok_or_else(|| format!("invalid --since date: {since_s}"))?;
        // A bare date parses at start-of-day; extend a date-only --until to
        // end-of-day so the window is inclusive of the whole final day.
        let until_raw =
            parse_date(until_s).ok_or_else(|| format!("invalid --until date: {until_s}"))?;
        let until = if until_s.len() <= 10 {
            end_of_day(until_raw)
        } else {
            until_raw
        };
        if until < since {
            return Err("--until must not be before --since".into());
        }
        let label = format!(
            "{} – {}",
            since.format("%Y-%m-%d"),
            until.format("%Y-%m-%d")
        );
        return Ok(ResolvedRange {
            since,
            until,
            label,
        });
    }

    if args.weekly {
        // The last 7 calendar days: six days ago at midnight → end of today.
        let since = start_of_day(now - chrono::Duration::days(6));
        return Ok(ResolvedRange {
            since,
            until: end_of_day(now),
            label: format!("week of {}", since.format("%Y-%m-%d")),
        });
    }

    // Default (`--daily` or nothing): the current calendar day, midnight → 23:59:59.
    Ok(ResolvedRange {
        since: start_of_day(now),
        until: end_of_day(now),
        label: now.format("%Y-%m-%d").to_string(),
    })
}

pub async fn run(args: DigestArgs, cfg: &Config, json: bool) -> ExitCode {
    let range = match resolve_range(&args, Local::now()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    if args.show {
        return run_show(&range, cfg, json).await;
    }

    // Generate: spawning path (a missing daemon is started). The daemon ACKs
    // `null` and rolls up in the background, like `phoneme summarize`.
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let req = Request::RerunPeriodDigest {
        since: range.since,
        until: range.until,
        label: range.label.clone(),
        model: args.model,
    };
    match client.send(req).await {
        Ok(value) => {
            if json {
                output::print_json(&value);
            } else {
                println!("period digest requested ({})", range.label);
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

/// `--show`: read the stored digest for the resolved window (observe-only path).
async fn run_show(range: &ResolvedRange, cfg: &Config, json: bool) -> ExitCode {
    let mut client = match Client::connect_observe(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let value = match client
        .send(Request::GetPeriodDigest { key: range.key() })
        .await
    {
        Ok(v) => v,
        Err(code) => return code,
    };
    if json {
        output::print_json(&value);
        return ExitCode::SUCCESS;
    }
    // A never-generated range yields `null` — a normal state, not an error.
    let digest: Option<PeriodDigest> = match serde_json::from_value(value) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: parsing period digest response: {e}");
            return ExitCode::FAILURE;
        }
    };
    match digest {
        Some(d) => {
            println!("{} ({} recordings)", d.label, d.source_count);
            if let Some(m) = d.digest_model.as_deref() {
                println!("model: {m}");
            }
            println!();
            println!("{}", d.digest);
        }
        None => println!("no digest yet for {}", range.label),
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn args(daily: bool, weekly: bool, since: Option<&str>, until: Option<&str>) -> DigestArgs {
        DigestArgs {
            show: false,
            daily,
            weekly,
            since: since.map(str::to_string),
            until: until.map(str::to_string),
            model: None,
        }
    }

    #[test]
    fn daily_snaps_to_the_full_calendar_day() {
        // Snapped to whole-day bounds (independent of the exact `now`) so the key
        // is stable for a later `--show` the same day.
        let now = Local.with_ymd_and_hms(2026, 6, 21, 14, 30, 0).unwrap();
        let r = resolve_range(&args(true, false, None, None), now).unwrap();
        assert_eq!(
            r.since,
            Local.with_ymd_and_hms(2026, 6, 21, 0, 0, 0).unwrap()
        );
        assert_eq!(
            r.until,
            Local.with_ymd_and_hms(2026, 6, 21, 23, 59, 59).unwrap()
        );
        assert_eq!(r.label, "2026-06-21");
    }

    #[test]
    fn key_is_stable_within_the_day_for_daily() {
        // Two different `now` instants on the same day → the same key, so
        // `--show` after `digest` hits the stored digest.
        let morning = Local.with_ymd_and_hms(2026, 6, 21, 8, 0, 0).unwrap();
        let evening = Local.with_ymd_and_hms(2026, 6, 21, 20, 0, 0).unwrap();
        let a = resolve_range(&args(true, false, None, None), morning).unwrap();
        let b = resolve_range(&args(true, false, None, None), evening).unwrap();
        assert_eq!(a.key(), b.key());
    }

    #[test]
    fn default_with_no_flags_is_daily() {
        let now = Local.with_ymd_and_hms(2026, 6, 21, 14, 30, 0).unwrap();
        let r = resolve_range(&args(false, false, None, None), now).unwrap();
        assert_eq!(
            r.since,
            Local.with_ymd_and_hms(2026, 6, 21, 0, 0, 0).unwrap()
        );
        assert_eq!(
            r.until,
            Local.with_ymd_and_hms(2026, 6, 21, 23, 59, 59).unwrap()
        );
    }

    #[test]
    fn weekly_is_last_seven_calendar_days() {
        let now = Local.with_ymd_and_hms(2026, 6, 21, 14, 30, 0).unwrap();
        let r = resolve_range(&args(false, true, None, None), now).unwrap();
        // Six days ago at midnight → end of today = a 7-calendar-day inclusive window.
        assert_eq!(
            r.since,
            Local.with_ymd_and_hms(2026, 6, 15, 0, 0, 0).unwrap()
        );
        assert_eq!(
            r.until,
            Local.with_ymd_and_hms(2026, 6, 21, 23, 59, 59).unwrap()
        );
        assert_eq!(r.label, "week of 2026-06-15");
    }

    #[test]
    fn custom_range_parses_bare_dates_inclusive_to_end_of_day() {
        let now = Local.with_ymd_and_hms(2026, 6, 21, 14, 30, 0).unwrap();
        let r = resolve_range(
            &args(false, false, Some("2026-06-15"), Some("2026-06-20")),
            now,
        )
        .unwrap();
        assert_eq!(
            r.since,
            Local.with_ymd_and_hms(2026, 6, 15, 0, 0, 0).unwrap()
        );
        // A bare --until covers the whole final day (inclusive).
        assert_eq!(
            r.until,
            Local.with_ymd_and_hms(2026, 6, 20, 23, 59, 59).unwrap()
        );
        assert_eq!(r.label, "2026-06-15 – 2026-06-20");
    }

    #[test]
    fn custom_range_rejects_until_before_since() {
        let now = Local.with_ymd_and_hms(2026, 6, 21, 14, 30, 0).unwrap();
        let err = resolve_range(
            &args(false, false, Some("2026-06-20"), Some("2026-06-15")),
            now,
        )
        .unwrap_err();
        assert!(err.contains("must not be before"), "got: {err}");
    }

    #[test]
    fn custom_range_rejects_malformed_date() {
        let now = Local.with_ymd_and_hms(2026, 6, 21, 14, 30, 0).unwrap();
        let err = resolve_range(
            &args(false, false, Some("not-a-date"), Some("2026-06-20")),
            now,
        )
        .unwrap_err();
        assert!(err.contains("invalid --since"), "got: {err}");
    }

    #[test]
    fn key_matches_the_daemon_format() {
        let r = ResolvedRange {
            since: Local.with_ymd_and_hms(2026, 6, 21, 0, 0, 0).unwrap(),
            until: Local.with_ymd_and_hms(2026, 6, 21, 23, 59, 59).unwrap(),
            label: "2026-06-21".into(),
        };
        let expected = format!("{}|{}", r.since.to_rfc3339(), r.until.to_rfc3339());
        assert_eq!(r.key(), expected);
    }
}
