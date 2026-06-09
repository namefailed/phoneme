use crate::args::ListArgs;
use crate::client::Client;
use crate::exit;
use crate::output;
use phoneme_core::{Config, ListFilter, Recording, RecordingStatus};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: ListArgs, cfg: &Config, json: bool) -> ExitCode {
    // `--semantic` short-circuits to an embedding search, reusing --limit.
    if let Some(query) = args.semantic.clone() {
        return crate::commands::search::run(
            crate::args::SearchArgs {
                query,
                limit: args.limit.map(|n| n as usize).unwrap_or(20),
            },
            cfg,
            json,
        )
        .await;
    }

    // Capture the type-filter before `build_filter` consumes `args`. Applied
    // client-side on `meeting_id` (single = none, meeting = present) to mirror
    // the GUI Library filter; the daemon's list shape stays unchanged.
    let kind = args.kind.clone();
    let tag = args.tag.clone();

    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    // Resolve `--tag` to a tag id: accept a numeric id directly, otherwise
    // look the name up against the tag list.
    let tag_id = match resolve_tag(&mut client, tag.as_deref()).await {
        Ok(t) => t,
        Err(code) => return code,
    };

    let filter = build_filter(args, tag_id);
    let value = match client.send(Request::ListRecordings { filter }).await {
        Ok(v) => v,
        Err(code) => return code,
    };
    let mut rows: Vec<Recording> = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: parsing list response: {e}");
            return ExitCode::from(exit::GENERIC_FAIL);
        }
    };
    match kind.as_deref() {
        Some("single") => rows.retain(|r| r.meeting_id.is_none()),
        Some("meeting") => rows.retain(|r| r.meeting_id.is_some()),
        _ => {}
    }
    if json {
        output::print_json_lines(&rows);
    } else {
        output::print_list_pretty(&rows);
    }
    ExitCode::SUCCESS
}

/// Resolve a `--tag` argument (numeric id or tag name) to a tag id.
async fn resolve_tag(
    client: &mut Client,
    tag: Option<&str>,
) -> Result<Option<i64>, ExitCode> {
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
        "transcribing" => Some(RecordingStatus::Transcribing),
        "hook_running" => Some(RecordingStatus::HookRunning),
        "done" => Some(RecordingStatus::Done),
        "transcribe_failed" => Some(RecordingStatus::TranscribeFailed),
        "hook_failed" => Some(RecordingStatus::HookFailed),
        _ => None,
    });
    let parse_date = |s: String| {
        chrono::DateTime::parse_from_rfc3339(&s)
            .ok()
            .map(|d| d.with_timezone(&chrono::Local))
    };
    let since = args.since.and_then(parse_date);
    let until = args.until.and_then(parse_date);
    ListFilter {
        limit: args.limit,
        offset: args.offset,
        since,
        status,
        search: args.search,
        tag_id,
        sort_desc: None,
        until,
    }
}
