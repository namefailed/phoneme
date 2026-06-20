//! `phoneme search <QUERY>` / `phoneme search --like <ID>` — semantic recall
//! over the library.
//!
//! Observe-only: search is inspection. A text query sends `SemanticSearch`
//! (hybrid embedding + FTS5 ranking); `--like` sends `MoreLikeThis` instead,
//! using the recording's already-stored vectors as the query — no embedding
//! happens, so it works even when the model isn't loaded. Both return the
//! same `[{recording, score}]` shape, rendered as a relevance-scored list
//! (clap enforces exactly one of query/--like). `--limit` caps results.

use crate::args::SearchArgs;
use crate::client::Client;
use crate::exit;
use crate::output;
use phoneme_core::types::ListKind;
use phoneme_core::{Config, ListFilter, Recording, RecordingId, RecordingStatus};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: SearchArgs, cfg: &Config, json: bool) -> ExitCode {
    let mut client = match Client::connect_observe(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    // `--like <ID>` is "more like this": the recording's stored vectors are the
    // query, so no text is embedded. Clap guarantees exactly one of query/--like
    // and that the scope flags (--tag/--status/--kind) never combine with --like.
    let request = if let Some(like) = args.like {
        let id = match RecordingId::parse(like.as_str()) {
            Some(id) => id,
            None => {
                eprintln!("error: '{like}' is not a valid recording id");
                return ExitCode::FAILURE;
            }
        };
        Request::MoreLikeThis {
            id,
            limit: args.limit,
        }
    } else {
        // S3: build an optional Library scope from --tag/--status/--kind. `None`
        // when no scope flag is given (today's unscoped behavior).
        let filter = match build_scope(&mut client, &args).await {
            Ok(f) => f,
            Err(code) => return code,
        };
        Request::SemanticSearch {
            query: args.query.unwrap_or_default(),
            limit: args.limit,
            filter,
        }
    };

    let value = match client.send(request).await {
        Ok(v) => v,
        Err(code) => return code,
    };

    if json {
        output::print_json(&value);
        return ExitCode::SUCCESS;
    }

    // The daemon returns [{ "recording": Recording, "score": f32 }, ...].
    let Some(arr) = value.as_array() else {
        println!("no results");
        return ExitCode::SUCCESS;
    };
    if arr.is_empty() {
        println!("no results");
        return ExitCode::SUCCESS;
    }
    for hit in arr {
        let score = hit.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0);
        if let Some(rec) = hit.get("recording") {
            if let Ok(r) = serde_json::from_value::<Recording>(rec.clone()) {
                let preview = match &r.transcript {
                    Some(t) if t.chars().count() > 70 => {
                        let s: String = t.chars().take(70).collect();
                        format!("{s}…")
                    }
                    Some(t) => t.clone(),
                    None => String::new(),
                };
                println!("{:.3}  {}  {}", score, r.id.as_str(), preview);
            }
        }
    }
    ExitCode::SUCCESS
}

/// Build the optional S3 scope filter from `--tag` / `--status` / `--kind`.
/// Returns `Ok(None)` when no scope flag is set (unscoped search). A `--tag`
/// name is resolved against the daemon's tag list (all tags, so a zero-recording
/// tag still scopes to its empty set rather than erroring).
async fn build_scope(
    client: &mut Client,
    args: &SearchArgs,
) -> Result<Option<ListFilter>, ExitCode> {
    if args.tag.is_none() && args.status.is_none() && args.kind.is_none() {
        return Ok(None);
    }

    let tag_id = match args.tag.as_deref() {
        None => None,
        Some(tag) => {
            if let Ok(id) = tag.parse::<i64>() {
                Some(id)
            } else {
                let value = client.send(Request::ListAllTags).await?;
                let tags: Vec<phoneme_core::tags::Tag> =
                    serde_json::from_value(value).map_err(|e| {
                        eprintln!("error: parsing tags list: {e}");
                        ExitCode::from(exit::GENERIC_FAIL)
                    })?;
                match tags.into_iter().find(|t| t.name == tag) {
                    Some(t) => Some(t.id),
                    None => {
                        eprintln!("error: tag '{tag}' not found");
                        return Err(ExitCode::from(exit::NOT_FOUND));
                    }
                }
            }
        }
    };

    let status = match args.status.as_deref() {
        None => None,
        Some(s) => match RecordingStatus::from_str_opt(s) {
            Some(st) => Some(st),
            None => {
                eprintln!("error: '{s}' is not a valid status");
                return Err(ExitCode::from(exit::GENERIC_FAIL));
            }
        },
    };

    let kind = args.kind.as_deref().and_then(|k| match k {
        "single" => Some(ListKind::Single),
        "meeting" => Some(ListKind::Meeting),
        _ => None,
    });

    Ok(Some(ListFilter {
        tag_id,
        status,
        kind,
        ..ListFilter::default()
    }))
}
