use crate::args::ListArgs;
use crate::client::Client;
use crate::exit;
use crate::output;
use phoneme_core::{Config, ListFilter, Recording, RecordingStatus};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: ListArgs, cfg: &Config, json: bool) -> ExitCode {
    // Capture the type-filter before `build_filter` consumes `args`. Applied
    // client-side on `meeting_id` (single = none, meeting = present) to mirror
    // the GUI Library filter; the daemon's list shape stays unchanged.
    let kind = args.kind.clone();
    let filter = build_filter(args);
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
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

fn build_filter(args: ListArgs) -> ListFilter {
    let status = args.status.as_deref().and_then(|s| match s {
        "recording" => Some(RecordingStatus::Recording),
        "transcribing" => Some(RecordingStatus::Transcribing),
        "hook_running" => Some(RecordingStatus::HookRunning),
        "done" => Some(RecordingStatus::Done),
        "transcribe_failed" => Some(RecordingStatus::TranscribeFailed),
        "hook_failed" => Some(RecordingStatus::HookFailed),
        _ => None,
    });
    let since = args.since.and_then(|s| {
        chrono::DateTime::parse_from_rfc3339(&s)
            .ok()
            .map(|d| d.with_timezone(&chrono::Local))
    });
    ListFilter {
        limit: args.limit,
        offset: args.offset,
        since,
        status,
        search: args.search,
        tag_id: None,
        sort_desc: None,
        until: None,
    }
}
