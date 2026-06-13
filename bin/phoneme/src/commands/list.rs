//! `phoneme list` — query the recording catalog.
//!
//! Observe-only (`Client::connect_observe`): listing is inspection, and "the
//! daemon is down" is a more useful answer than silently starting one.
//! Builds a `ListFilter` from the flags (`--limit/--offset` pagination,
//! `--since/--until` dates, `--status`, `--search` FTS5, `--kind`
//! single/meeting) and sends `ListRecordings`; `--tag` accepts an id or a
//! name and is resolved against `ListTags`/`ListAllTags` first. The `kind`
//! filter is applied in SQL by the daemon — before LIMIT/OFFSET — so pages
//! stay full. `--semantic <QUERY>` short-circuits the whole flow into
//! `commands::search` (a `SemanticSearch` request) reusing `--limit`.

use crate::args::ListArgs;
use crate::client::Client;
use crate::exit;
use crate::output;
use phoneme_core::types::ListKind;
use phoneme_core::{Config, ListFilter, Recording, RecordingStatus};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: ListArgs, cfg: &Config, json: bool) -> ExitCode {
    // `--semantic` short-circuits to an embedding search, reusing --limit.
    if let Some(query) = args.semantic.clone() {
        return crate::commands::search::run(
            crate::args::SearchArgs {
                query: Some(query),
                like: None,
                limit: args.limit.map(|n| n as usize).unwrap_or(20),
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
    let filter = build_filter(args, tag_id);
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

/// Resolve a `--tag` argument (numeric id or tag name) to a tag id.
async fn resolve_tag(client: &mut Client, tag: Option<&str>) -> Result<Option<i64>, ExitCode> {
    let Some(tag) = tag else { return Ok(None) };
    if let Ok(id) = tag.parse::<i64>() {
        return Ok(Some(id));
    }
    let value = client.send(Request::ListTags).await?;
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

fn build_filter(args: ListArgs, tag_id: Option<i64>) -> ListFilter {
    let status = args.status.as_deref().and_then(|s| match s {
        "recording" => Some(RecordingStatus::Recording),
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
    let parse_date = |s: String| {
        chrono::DateTime::parse_from_rfc3339(&s)
            .ok()
            .map(|d| d.with_timezone(&chrono::Local))
    };
    let since = args.since.and_then(parse_date);
    let until = args.until.and_then(parse_date);
    // `kind` is applied in SQL (before LIMIT/OFFSET) so pagination stays correct.
    let kind = args.kind.as_deref().and_then(|k| match k {
        "single" => Some(ListKind::Single),
        "meeting" => Some(ListKind::Meeting),
        _ => None, // "all" or unrecognised → no filter
    });
    ListFilter {
        limit: args.limit,
        offset: args.offset,
        since,
        status,
        search: args.search,
        tag_id,
        sort_desc: None,
        until,
        kind,
        favorite: None, // no CLI flag for this yet
    }
}
