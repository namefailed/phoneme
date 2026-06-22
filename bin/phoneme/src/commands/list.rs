//! `phoneme list` — query the recording catalog.
//!
//! Observe-only (`Client::connect_observe`): listing is inspection, and "the
//! daemon is down" is a more useful answer than silently starting one.
//! Builds a `ListFilter` from the flags (`--limit/--offset` pagination,
//! `--since/--until` dates, `--status`, `--search` FTS5, `--kind`
//! single/meeting) and sends `ListRecordings`; `--tag` accepts an id or a
//! name and is resolved against `ListAllTags` first (uses all tags, including
//! orphaned ones, so `--tag <name>` on a tag with zero recordings returns the
//! correct empty list rather than "not found"). The `kind`
//! filter is applied in SQL by the daemon — before LIMIT/OFFSET — so pages
//! stay full. `--semantic <QUERY>` short-circuits the whole flow into
//! `commands::search` (a `SemanticSearch` request) reusing `--limit`.

use crate::args::ListArgs;
use crate::client::Client;
use crate::exit;
use crate::output;
use phoneme_core::types::ListKind;
use phoneme_core::{Config, ListFilter, Recording, RecordingStatus, SavedSearch};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: ListArgs, cfg: &Config, json: bool) -> ExitCode {
    // `--saved` runs (or lists) saved searches server-side, short-circuiting the
    // normal filter flow. An empty value (`--saved` with no id) lists the stored
    // searches; an id executes that one. Other list filters are ignored here.
    if let Some(saved) = args.saved.clone() {
        return run_saved(saved, cfg, json).await;
    }

    // `--semantic` short-circuits to an embedding search, reusing --limit.
    if let Some(query) = args.semantic.clone() {
        return crate::commands::search::run(
            crate::args::SearchArgs {
                query: Some(query),
                like: None,
                limit: args.limit.map(|n| n as usize).unwrap_or(20),
                // `list --semantic` forwards the existing list scope flags so a
                // scoped semantic search works via either entry point (S3).
                tag: args.tag.clone(),
                status: args.status.clone(),
                kind: args.kind.clone(),
            },
            cfg,
            json,
        )
        .await;
    }

    let tag = args.tag.clone();

    let mut client = match Client::connect_observe(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    // Resolve `--tag` to a tag id: accept a numeric id directly, otherwise
    // look the name up against the tag list.
    let tag_id = match resolve_tag(&mut client, tag.as_deref()).await {
        Ok(t) => t,
        Err(code) => return code,
    };

    // The `kind` filter is now applied in SQL by the daemon (before LIMIT /
    // OFFSET) so pagination works correctly — filtering client-side after
    // pagination caused pages to be mostly empty for the non-default kind.
    let filter = match build_filter(args, tag_id) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let value = match client.send(Request::ListRecordings { filter }).await {
        Ok(v) => v,
        Err(code) => return code,
    };
    let rows: Vec<Recording> = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: parsing list response: {e}");
            return ExitCode::from(exit::GENERIC_FAIL);
        }
    };
    if json {
        output::print_json_lines(&rows);
    } else {
        output::print_list_pretty(&rows);
    }
    ExitCode::SUCCESS
}

/// `phoneme list --saved [ID]`: run a stored saved search by id, or — with no
/// id — list the available saved searches (id + name) so the user can pick one.
async fn run_saved(id: String, cfg: &Config, json: bool) -> ExitCode {
    let mut client = match Client::connect_observe(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    // No id given (`--saved` alone): list the saved searches instead of running.
    if id.is_empty() {
        let value = match client.send(Request::ListSavedSearches).await {
            Ok(v) => v,
            Err(code) => return code,
        };
        let searches: Vec<SavedSearch> = match serde_json::from_value(value) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: parsing saved searches: {e}");
                return ExitCode::from(exit::GENERIC_FAIL);
            }
        };
        if json {
            output::print_json_lines(&searches);
        } else if searches.is_empty() {
            println!("no saved searches");
        } else {
            for s in &searches {
                println!("{}  {}", s.id, s.name);
            }
        }
        return ExitCode::SUCCESS;
    }

    let value = match client.send(Request::RunSavedSearch { id }).await {
        Ok(v) => v,
        Err(code) => return code,
    };
    let rows: Vec<Recording> = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: parsing list response: {e}");
            return ExitCode::from(exit::GENERIC_FAIL);
        }
    };
    if json {
        output::print_json_lines(&rows);
    } else {
        output::print_list_pretty(&rows);
    }
    ExitCode::SUCCESS
}

/// Resolve a `--tag` argument (numeric id or tag name) to a tag id.
async fn resolve_tag(client: &mut Client, tag: Option<&str>) -> Result<Option<i64>, ExitCode> {
    let Some(tag) = tag else { return Ok(None) };
    if let Ok(id) = tag.parse::<i64>() {
        return Ok(Some(id));
    }
    let value = client.send(Request::ListAllTags).await?;
    let tags: Vec<phoneme_core::tags::Tag> = serde_json::from_value(value).map_err(|e| {
        eprintln!("error: parsing tags list: {e}");
        ExitCode::from(exit::GENERIC_FAIL)
    })?;
    match tags.into_iter().find(|t| t.name == tag) {
        Some(t) => Ok(Some(t.id)),
        None => {
            eprintln!("error: tag '{tag}' not found");
            Err(ExitCode::from(exit::NOT_FOUND))
        }
    }
}

fn build_filter(args: ListArgs, tag_id: Option<i64>) -> Result<ListFilter, ExitCode> {
    let status = args.status.as_deref().and_then(|s| match s {
        "recording" => Some(RecordingStatus::Recording),
        "paused" => Some(RecordingStatus::Paused),
        "queued" => Some(RecordingStatus::Queued),
        "transcribing" => Some(RecordingStatus::Transcribing),
        "cleaning_up" => Some(RecordingStatus::CleaningUp),
        "summarizing" => Some(RecordingStatus::Summarizing),
        "tagging" => Some(RecordingStatus::Tagging),
        "hook_running" => Some(RecordingStatus::HookRunning),
        "done" => Some(RecordingStatus::Done),
        "transcribe_failed" => Some(RecordingStatus::TranscribeFailed),
        "hook_failed" => Some(RecordingStatus::HookFailed),
        "cleanup_failed" => Some(RecordingStatus::CleanupFailed),
        "summarize_failed" => Some(RecordingStatus::SummarizeFailed),
        "title_failed" => Some(RecordingStatus::TitleFailed),
        "tag_failed" => Some(RecordingStatus::TagFailed),
        "cancelled" => Some(RecordingStatus::Cancelled),
        _ => None,
    });
    // The flag's help advertises bare dates (e.g. 2026-05-19), but a full
    // RFC 3339 timestamp is also accepted. Try RFC 3339 first; on failure fall
    // back to a date-only parse interpreted at local start-of-day, so the
    // documented date form actually filters instead of being silently dropped.
    //
    // `end_of_day` snaps the bare-date form to 23:59:59 for `--until`: the daemon
    // applies it as `started_at <= ?`, so a bare `--until 2026-05-19` at
    // start-of-day would drop everything recorded that day (off-by-one vs the
    // documented "inclusive" bound). `--since` keeps start-of-day, which is the
    // right inclusive lower bound. Explicit RFC 3339 timestamps are honoured as-is.
    let parse_date = |s: String, end_of_day: bool| {
        if let Ok(d) = chrono::DateTime::parse_from_rfc3339(&s) {
            return Some(d.with_timezone(&chrono::Local));
        }
        let time = if end_of_day {
            (23, 59, 59)
        } else {
            (0, 0, 0)
        };
        chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(time.0, time.1, time.2))
            .and_then(|naive| {
                use chrono::TimeZone;
                chrono::Local.from_local_datetime(&naive).single()
            })
    };
    // A *present* but unparseable date is a usage error, not "no filter": silently
    // dropping it would widen the query to the whole library — the same footgun
    // --status / --kind are clap-validated against (see args.rs). The flags carry
    // no clap value_parser (the format is too lax for a fixed set), so guard here.
    let parse_flag = |name: &str, v: Option<String>, end_of_day: bool| -> Result<Option<_>, ExitCode> {
        match v {
            None => Ok(None),
            Some(s) => match parse_date(s.clone(), end_of_day) {
                Some(d) => Ok(Some(d)),
                None => {
                    eprintln!("error: could not parse {name} '{s}' (expected e.g. 2026-05-19)");
                    Err(ExitCode::from(exit::USAGE_ERROR))
                }
            },
        }
    };
    let since = parse_flag("--since", args.since, false)?;
    let until = parse_flag("--until", args.until, true)?;
    // `kind` is applied in SQL (before LIMIT/OFFSET) so pagination stays correct.
    let kind = args.kind.as_deref().and_then(|k| match k {
        "single" => Some(ListKind::Single),
        "meeting" => Some(ListKind::Meeting),
        _ => None, // "all" or unrecognised → no filter
    });
    Ok(ListFilter {
        limit: args.limit,
        offset: args.offset,
        since,
        status,
        search: args.search,
        tag_id,
        sort_desc: None,
        until,
        kind,
        favorite: None,             // no CLI flag for this yet
        pinned: None,               // no CLI flag for this yet
        in_place: None,             // no CLI flag for this yet
        tagged: None,               // no CLI flag for this yet
        entity_value: None,         // no CLI flag for this yet (see `phoneme entities`)
        entity_kind: None,          // no CLI flag for this yet
        low_confidence_below: None, // no CLI flag for this yet
        task_state: None,           // no CLI flag for this yet (see `phoneme tasks`)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    /// A `ListArgs` with everything off, so a test can set just the field it cares
    /// about without spelling out the whole struct.
    fn empty_args() -> ListArgs {
        ListArgs {
            limit: None,
            offset: None,
            since: None,
            until: None,
            status: None,
            tag: None,
            search: None,
            semantic: None,
            kind: None,
            saved: None,
        }
    }

    // `--until <day>` is documented as an inclusive upper bound, but the daemon
    // applies it as `started_at <= ?`. A bare date must therefore resolve to
    // end-of-day, or a same-day recording (e.g. 09:00) gets dropped.
    #[test]
    fn until_bare_date_snaps_to_end_of_day() {
        let mut args = empty_args();
        args.until = Some("2026-05-19".into());
        let filter = build_filter(args, None).expect("filter builds");
        let until = filter.until.expect("until parsed");
        assert_eq!((until.hour(), until.minute(), until.second()), (23, 59, 59));

        // The whole named day is included: a 09:00 recording satisfies the bound.
        use chrono::TimeZone;
        let same_day = chrono::Local
            .with_ymd_and_hms(2026, 5, 19, 9, 0, 0)
            .single()
            .unwrap();
        assert!(same_day <= until);
    }

    // `--since` is the lower bound, so start-of-day is correct — leave it alone.
    #[test]
    fn since_bare_date_stays_start_of_day() {
        let mut args = empty_args();
        args.since = Some("2026-05-19".into());
        let filter = build_filter(args, None).expect("filter builds");
        let since = filter.since.expect("since parsed");
        assert_eq!((since.hour(), since.minute(), since.second()), (0, 0, 0));
    }
}
