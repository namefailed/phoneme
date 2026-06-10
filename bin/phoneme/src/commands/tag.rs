use crate::args::{TagAction, TagArgs};
use crate::client;
use crate::output;
use phoneme_core::Config;
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: TagArgs, cfg: &Config, is_json: bool) -> ExitCode {
    let mut conn = match client::Client::connect(cfg).await {
        Ok(c) => c,
        Err(e) => return e,
    };

    match args.action {
        TagAction::List { all } => {
            // `--all` includes orphaned tags (mirrors the GUI Tag Manager); the
            // default list only returns tags attached to a recording.
            let req = if all {
                Request::ListAllTags
            } else {
                Request::ListTags
            };
            match conn.send(req).await {
                Ok(val) => {
                    if is_json {
                        output::print_json(&val);
                    } else if let Some(arr) = val.as_array() {
                        for t in arr {
                            if let (Some(id), Some(name)) = (t.get("id"), t.get("name")) {
                                println!("{}: {}", id, name);
                            }
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => e,
            }
        }
        TagAction::Add { name, color } => match conn.send(Request::AddTag { name, color }).await {
            Ok(val) => {
                if is_json {
                    output::print_json(&val);
                } else {
                    println!("added tag");
                }
                ExitCode::SUCCESS
            }
            Err(e) => e,
        },
        TagAction::Update { id, name, color } => {
            match conn.send(Request::UpdateTag { id, name, color }).await {
                Ok(val) => {
                    if is_json {
                        output::print_json(&val);
                    } else {
                        println!("updated tag");
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => e,
            }
        }
        TagAction::Delete { id } => match conn.send(Request::DeleteTag { id }).await {
            Ok(_) => ExitCode::SUCCESS,
            Err(e) => e,
        },
        TagAction::Attach { recording_id, tag } => {
            let Some(rid) = phoneme_core::id::RecordingId::parse(&recording_id) else {
                eprintln!("error: invalid recording ID '{}'", recording_id);
                return ExitCode::FAILURE;
            };
            let tag_id = match resolve_tag(&mut conn, &tag).await {
                Ok(id) => id,
                Err(code) => return code,
            };
            match conn
                .send(Request::AttachTag {
                    recording_id: rid,
                    tag_id,
                })
                .await
            {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => e,
            }
        }
        TagAction::Detach { recording_id, tag } => {
            let Some(rid) = phoneme_core::id::RecordingId::parse(&recording_id) else {
                eprintln!("error: invalid recording ID '{}'", recording_id);
                return ExitCode::FAILURE;
            };
            let tag_id = match resolve_tag(&mut conn, &tag).await {
                Ok(id) => id,
                Err(code) => return code,
            };
            match conn
                .send(Request::DetachTag {
                    recording_id: rid,
                    tag_id,
                })
                .await
            {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => e,
            }
        }
        TagAction::For { recording_id } => {
            let Some(rid) = phoneme_core::id::RecordingId::parse(&recording_id) else {
                eprintln!("error: invalid recording ID '{}'", recording_id);
                return ExitCode::FAILURE;
            };
            match conn.send(Request::TagsFor { recording_id: rid }).await {
                Ok(val) => {
                    if is_json {
                        output::print_json(&val);
                    } else if let Some(arr) = val.as_array() {
                        for t in arr {
                            if let (Some(id), Some(name)) = (t.get("id"), t.get("name")) {
                                println!("{}: {}", id, name);
                            }
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => e,
            }
        }
        TagAction::Usage => match conn.send(Request::TagUsageCounts).await {
            Ok(val) => {
                if is_json {
                    output::print_json(&val);
                } else if let Some(map) = val.as_object() {
                    // The daemon keys usage counts by tag id; resolve names so
                    // the human-readable output isn't just opaque ids.
                    let names = tag_names(&mut conn).await;
                    for (id, count) in map {
                        let name = id
                            .parse::<i64>()
                            .ok()
                            .and_then(|i| names.get(&i).cloned())
                            .unwrap_or_default();
                        println!("{id}: {count}  {name}");
                    }
                }
                ExitCode::SUCCESS
            }
            Err(e) => e,
        },
        TagAction::Merge { from, into } => {
            let from_id = match resolve_tag(&mut conn, &from).await {
                Ok(id) => id,
                Err(code) => return code,
            };
            let into_id = match resolve_tag(&mut conn, &into).await {
                Ok(id) => id,
                Err(code) => return code,
            };
            match conn.send(Request::MergeTags { from_id, into_id }).await {
                Ok(_) => {
                    if !is_json {
                        println!("merged tag {from_id} into {into_id}");
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => e,
            }
        }
    }
}

/// Resolve a tag reference (numeric id or name) to a tag id. Names are looked up
/// against the full tag list (including orphans) so merging/attaching by name
/// works regardless of whether the tag is currently in use.
async fn resolve_tag(conn: &mut client::Client, tag: &str) -> Result<i64, ExitCode> {
    if let Ok(id) = tag.parse::<i64>() {
        return Ok(id);
    }
    let val = conn.send(Request::ListAllTags).await?;
    let tags: Vec<phoneme_core::tags::Tag> = serde_json::from_value(val).map_err(|e| {
        eprintln!("error parsing tags list: {e}");
        ExitCode::FAILURE
    })?;
    match tags.into_iter().find(|t| t.name == tag) {
        Some(t) => Ok(t.id),
        None => {
            eprintln!("error: tag '{}' not found", tag);
            Err(ExitCode::from(crate::exit::NOT_FOUND))
        }
    }
}

/// Best-effort id→name map of all tags, for decorating `usage` output. On any
/// failure we just return an empty map (names are a nicety, not load-bearing).
async fn tag_names(conn: &mut client::Client) -> std::collections::HashMap<i64, String> {
    match conn.send(Request::ListAllTags).await {
        Ok(val) => serde_json::from_value::<Vec<phoneme_core::tags::Tag>>(val)
            .map(|tags| tags.into_iter().map(|t| (t.id, t.name)).collect())
            .unwrap_or_default(),
        Err(_) => std::collections::HashMap::new(),
    }
}
