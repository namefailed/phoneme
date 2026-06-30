//! `phoneme tag …` — manage tags from the terminal (the CLI face of the GUI
//! Tag Manager).
//!
//! Each subcommand maps 1:1 to a tag IPC request: `list` (`ListTags`, or
//! `ListAllTags` with `--all` to include orphans), `add` (`AddTag`),
//! `update` (`UpdateTag`), `delete` (`DeleteTag`), `attach`/`detach`
//! (`AttachTag`/`DetachTag`), `for` (`TagsFor`), `usage` (`TagUsageCounts`),
//! `merge` (`MergeTags`), `clear-suggestions` (`ClearAllTagSuggestions`), and
//! `suggestions <id>` — list one recording's pending auto-tag proposals (read
//! from the recording DTO via `GetRecording`), or `--approve <name>` /
//! `--dismiss <name>` to act on one (`ApproveTagSuggestion` /
//! `DismissTagSuggestion`). Subcommands taking a tag accept an id or a name
//! (names are resolved against the tag list first). Uses the spawning path
//! throughout — tag edits need a daemon, and listing through the same
//! connection keeps the command simple.

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
        TagAction::Suggestions {
            recording_id,
            approve,
            dismiss,
        } => {
            let Some(rid) = phoneme_core::id::RecordingId::parse(&recording_id) else {
                eprintln!("error: invalid recording ID '{}'", recording_id);
                return ExitCode::FAILURE;
            };
            // --approve / --dismiss act on one named suggestion; with neither,
            // list the recording's current pending suggestions.
            if let Some(name) = approve {
                match conn
                    .send(Request::ApproveTagSuggestion {
                        id: rid,
                        name: name.clone(),
                    })
                    .await
                {
                    Ok(val) => {
                        if is_json {
                            output::print_json(&val);
                        } else {
                            println!("approved '{name}'");
                        }
                        ExitCode::SUCCESS
                    }
                    Err(code) => code,
                }
            } else if let Some(name) = dismiss {
                match conn
                    .send(Request::DismissTagSuggestion {
                        id: rid,
                        name: name.clone(),
                    })
                    .await
                {
                    Ok(_) => {
                        if !is_json {
                            println!("dismissed '{name}'");
                        }
                        ExitCode::SUCCESS
                    }
                    Err(code) => code,
                }
            } else {
                // List: the suggestions live on the recording DTO.
                match conn.send(Request::GetRecording { id: rid }).await {
                    Ok(val) => {
                        let suggestions = val
                            .get("tag_suggestions")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();
                        if is_json {
                            output::print_json(&serde_json::Value::Array(suggestions));
                        } else if suggestions.is_empty() {
                            println!("no pending tag suggestions");
                        } else {
                            for s in &suggestions {
                                if let Some(name) = s.as_str() {
                                    println!("{name}");
                                }
                            }
                        }
                        ExitCode::SUCCESS
                    }
                    Err(code) => code,
                }
            }
        }
        TagAction::ClearSuggestions => match conn.send(Request::ClearAllTagSuggestions).await {
            Ok(v) => {
                let n = v.get("cleared").and_then(|c| c.as_u64()).unwrap_or(0);
                if is_json {
                    output::print_json(&v);
                } else if n == 0 {
                    println!("no pending suggestions to clear");
                } else {
                    println!("cleared suggestions on {n} recording(s)");
                }
                ExitCode::SUCCESS
            }
            Err(code) => code,
        },
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::MockDaemon;
    use phoneme_core::id::RecordingId;
    use phoneme_ipc::Response;
    use std::time::Duration;

    async fn run_tag(
        action: TagAction,
        responder: impl Fn(&Request) -> Response + Send + Sync + 'static,
    ) -> (ExitCode, Vec<Request>) {
        let mock = MockDaemon::spawn("tag", responder);
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();
        let code =
            tokio::time::timeout(Duration::from_secs(5), run(TagArgs { action }, &cfg, false))
                .await
                .expect("tag must return promptly");
        (code, mock.received())
    }

    #[tokio::test]
    async fn suggestions_approve_sends_approve_request() {
        let id = RecordingId::new();
        let (code, reqs) = run_tag(
            TagAction::Suggestions {
                recording_id: id.to_string(),
                approve: Some("work".into()),
                dismiss: None,
            },
            |_req| Response::Ok(serde_json::json!({ "id": 1, "name": "work", "color": null })),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::ApproveTagSuggestion {
                id,
                name: "work".into()
            }]
        );
    }

    #[tokio::test]
    async fn suggestions_dismiss_sends_dismiss_request() {
        let id = RecordingId::new();
        let (code, reqs) = run_tag(
            TagAction::Suggestions {
                recording_id: id.to_string(),
                approve: None,
                dismiss: Some("spam".into()),
            },
            |_req| Response::Ok(serde_json::Value::Null),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::DismissTagSuggestion {
                id,
                name: "spam".into()
            }]
        );
    }

    #[tokio::test]
    async fn suggestions_list_fetches_the_recording() {
        let id = RecordingId::new();
        let (code, reqs) = run_tag(
            TagAction::Suggestions {
                recording_id: id.to_string(),
                approve: None,
                dismiss: None,
            },
            |_req| Response::Ok(serde_json::json!({ "tag_suggestions": ["work", "rust"] })),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::GetRecording { id }]);
    }

    /// `tag list --all` must send `ListAllTags` (includes orphans), not the
    /// default `ListTags`. A regression that swapped the two would pass without
    /// this assertion.
    #[tokio::test]
    async fn list_all_sends_list_all_tags() {
        let (code, reqs) = run_tag(TagAction::List { all: true }, |_req| {
            Response::Ok(serde_json::json!([]))
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::ListAllTags]);
    }

    /// `tag list` (no `--all`) must send `ListTags` (attached-only).
    #[tokio::test]
    async fn list_without_all_sends_list_tags() {
        let (code, reqs) = run_tag(TagAction::List { all: false }, |_req| {
            Response::Ok(serde_json::json!([]))
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::ListTags]);
    }

    /// `tag merge <from-name> <into-name>` resolves *both* endpoints by name via
    /// `ListAllTags`, then sends `MergeTags` with the resolved ids in the right
    /// from/into slots.
    #[tokio::test]
    async fn merge_resolves_both_names_to_ids() {
        let (code, reqs) = run_tag(
            TagAction::Merge {
                from: "work".into(),
                into: "Work".into(),
            },
            |req| match req {
                Request::ListAllTags => Response::Ok(serde_json::json!([
                    { "id": 11, "name": "work", "color": null },
                    { "id": 22, "name": "Work", "color": null },
                ])),
                _ => Response::Ok(serde_json::Value::Null),
            },
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        // Two name lookups (one per endpoint) then the merge with from=11 → into=22.
        assert_eq!(
            reqs,
            vec![
                Request::ListAllTags,
                Request::ListAllTags,
                Request::MergeTags {
                    from_id: 11,
                    into_id: 22,
                },
            ]
        );
    }

    /// A numeric tag id skips the name-lookup fast path: `tag merge 3 5` sends
    /// `MergeTags` directly with no `ListAllTags` round trips.
    #[tokio::test]
    async fn merge_with_numeric_ids_skips_lookup() {
        let (code, reqs) = run_tag(
            TagAction::Merge {
                from: "3".into(),
                into: "5".into(),
            },
            |_req| Response::Ok(serde_json::Value::Null),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::MergeTags {
                from_id: 3,
                into_id: 5,
            }]
        );
    }

    /// `tag attach <rec> <name>` resolves the name to its id and sends
    /// `AttachTag` with the resolved id (after the `ListAllTags` lookup).
    #[tokio::test]
    async fn attach_resolves_name_then_sends_attach() {
        let id = RecordingId::new();
        let (code, reqs) = run_tag(
            TagAction::Attach {
                recording_id: id.to_string(),
                tag: "rust".into(),
            },
            |req| match req {
                Request::ListAllTags => Response::Ok(serde_json::json!([
                    { "id": 7, "name": "rust", "color": null },
                ])),
                _ => Response::Ok(serde_json::Value::Null),
            },
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![
                Request::ListAllTags,
                Request::AttachTag {
                    recording_id: id,
                    tag_id: 7,
                },
            ]
        );
    }

    /// `tag detach <rec> <id>` with a numeric id sends `DetachTag` directly.
    #[tokio::test]
    async fn detach_with_numeric_id_sends_detach() {
        let id = RecordingId::new();
        let (code, reqs) = run_tag(
            TagAction::Detach {
                recording_id: id.to_string(),
                tag: "9".into(),
            },
            |_req| Response::Ok(serde_json::Value::Null),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::DetachTag {
                recording_id: id,
                tag_id: 9,
            }]
        );
    }

    /// An unknown tag name must exit `NOT_FOUND` (7) and send NO mutation — only
    /// the `ListAllTags` lookup that failed to resolve. Guards the "resolve to
    /// the wrong/absent id then mutate anyway" regression.
    #[tokio::test]
    async fn attach_unknown_name_exits_not_found_without_mutating() {
        let id = RecordingId::new();
        let (code, reqs) = run_tag(
            TagAction::Attach {
                recording_id: id.to_string(),
                tag: "nope".into(),
            },
            |_req| {
                // Tag list contains a different name, so "nope" never resolves.
                Response::Ok(serde_json::json!([
                    { "id": 1, "name": "work", "color": null },
                ]))
            },
        )
        .await;
        assert_eq!(
            format!("{code:?}"),
            format!("{:?}", ExitCode::from(crate::exit::NOT_FOUND))
        );
        // Only the lookup happened; no AttachTag was sent.
        assert_eq!(reqs, vec![Request::ListAllTags]);
    }

    /// `tag usage` sends `TagUsageCounts`, then (for the human output) looks up
    /// names via `ListAllTags` to decorate the ids.
    #[tokio::test]
    async fn usage_sends_counts_then_resolves_names() {
        let (code, reqs) = run_tag(TagAction::Usage, |req| match req {
            Request::TagUsageCounts => Response::Ok(serde_json::json!({ "1": 3, "2": 0 })),
            Request::ListAllTags => Response::Ok(serde_json::json!([
                { "id": 1, "name": "work", "color": null },
                { "id": 2, "name": "rust", "color": null },
            ])),
            _ => Response::Ok(serde_json::Value::Null),
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::TagUsageCounts, Request::ListAllTags]);
    }
}
