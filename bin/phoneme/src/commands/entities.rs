//! `phoneme entities` — list the cross-recording entity facet (the CLI face of
//! the GUI sidebar's browse-by-entity surface).
//!
//! Observe-only (`Client::connect_observe`): listing is inspection, like
//! `phoneme list` / `phoneme tag list`, so "the daemon is down" is a more useful
//! answer than silently starting one. Sends `ListAllEntities` and prints the
//! distinct `(kind, value)` entities with their recording counts, grouped by
//! kind; `--kind` filters to one class client-side. To then *list the
//! recordings* for one entity, pass its value to `phoneme list` (the entity
//! filter rides the list query) — this command is the facet, the counterpart of
//! `phoneme tag list`.

use crate::args::{EntitiesAction, EntitiesArgs};
use crate::client::Client;
use crate::exit;
use crate::output;
use phoneme_core::{Config, EntityFacet, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: EntitiesArgs, cfg: &Config, json: bool) -> ExitCode {
    match args.action {
        Some(EntitiesAction::Add { id, kind, value }) => add(&id, kind, value, cfg, json).await,
        Some(EntitiesAction::Edit {
            id,
            kind,
            value,
            to_kind,
            to_value,
        }) => edit(&id, kind, value, to_kind, to_value, cfg, json).await,
        Some(EntitiesAction::Delete { id, kind, value }) => {
            delete(&id, kind, value, cfg, json).await
        }
        Some(EntitiesAction::Merge {
            kind,
            to_value,
            from_values,
        }) => merge(kind, to_value, from_values, cfg, json).await,
        None => list(args.kind, cfg, json).await,
    }
}

/// Parse a recording id or print a clear error + the FAILURE code.
fn parse_recording_id(id: &str) -> Result<RecordingId, ExitCode> {
    RecordingId::parse(id).ok_or_else(|| {
        eprintln!("error: '{id}' is not a valid recording id");
        ExitCode::FAILURE
    })
}

/// `phoneme entities [--kind K]` — list the cross-recording entity facet.
async fn list(kind_filter: Option<String>, cfg: &Config, json: bool) -> ExitCode {
    let mut client = match Client::connect_observe(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    let value = match client.send(Request::ListAllEntities).await {
        Ok(v) => v,
        Err(code) => return code,
    };
    let mut facets: Vec<EntityFacet> = match serde_json::from_value(value) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: parsing entities list: {e}");
            return ExitCode::from(exit::GENERIC_FAIL);
        }
    };

    // `--kind` narrows to one class (person/org/topic/term); the daemon returns
    // every kind, so the filter is applied here.
    if let Some(kind) = kind_filter.as_deref() {
        facets.retain(|f| f.kind == kind);
    }

    if json {
        output::print_json_lines(&facets);
    } else if facets.is_empty() {
        println!("no entities yet (extract them with `phoneme suggest-entities <id>`)");
    } else {
        // The daemon already kind- then value-sorts, so grouping is a simple
        // run over the ordered list: print the kind header when it changes.
        let mut current_kind: Option<&str> = None;
        for f in &facets {
            if current_kind != Some(f.kind.as_str()) {
                println!("{}:", f.kind);
                current_kind = Some(f.kind.as_str());
            }
            println!("  {}  ({})", f.value, f.count);
        }
    }
    ExitCode::SUCCESS
}

/// `phoneme entities add <RECORDING_ID> <KIND> <VALUE>` — add a manual entity.
async fn add(id: &str, kind: String, value: String, cfg: &Config, json: bool) -> ExitCode {
    let id = match parse_recording_id(id) {
        Ok(id) => id,
        Err(code) => return code,
    };
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.send(Request::AddEntity { id, kind, value }).await {
        Ok(_) => {
            if !json {
                println!("entity added");
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

/// `phoneme entities edit <RECORDING_ID> <KIND> <VALUE> [--to-kind K] [--to-value V]`.
/// Keyed by the current `(kind, value)`; an omitted `--to-*` keeps that field.
async fn edit(
    id: &str,
    kind: String,
    value: String,
    to_kind: Option<String>,
    to_value: Option<String>,
    cfg: &Config,
    json: bool,
) -> ExitCode {
    if to_kind.is_none() && to_value.is_none() {
        eprintln!("error: pass --to-kind and/or --to-value to change something");
        return ExitCode::FAILURE;
    }
    let id = match parse_recording_id(id) {
        Ok(id) => id,
        Err(code) => return code,
    };
    let new_kind = to_kind.unwrap_or_else(|| kind.clone());
    let new_value = to_value.unwrap_or_else(|| value.clone());
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client
        .send(Request::UpdateEntity {
            id,
            kind,
            value,
            new_kind,
            new_value,
        })
        .await
    {
        Ok(_) => {
            if !json {
                println!("entity updated");
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

/// `phoneme entities delete <RECORDING_ID> <KIND> <VALUE>` — remove one entity.
async fn delete(id: &str, kind: String, value: String, cfg: &Config, json: bool) -> ExitCode {
    let id = match parse_recording_id(id) {
        Ok(id) => id,
        Err(code) => return code,
    };
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.send(Request::DeleteEntity { id, kind, value }).await {
        Ok(_) => {
            if !json {
                println!("entity deleted");
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

/// `phoneme entities merge <KIND> <TO_VALUE> <FROM_VALUE...>` — library-wide
/// fold of variant values into one canonical value.
async fn merge(
    kind: String,
    to_value: String,
    from_values: Vec<String>,
    cfg: &Config,
    json: bool,
) -> ExitCode {
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let count = from_values.len();
    match client
        .send(Request::MergeEntities {
            kind,
            from_values,
            to_value: to_value.clone(),
        })
        .await
    {
        Ok(_) => {
            if !json {
                println!(
                    "merged {count} variant{} into \"{to_value}\"",
                    if count == 1 { "" } else { "s" }
                );
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::MockDaemon;
    use phoneme_ipc::Response;
    use std::time::Duration;

    async fn run_entities(
        args: EntitiesArgs,
        responder: impl Fn(&Request) -> Response + Send + Sync + 'static,
    ) -> (ExitCode, Vec<Request>) {
        let mock = MockDaemon::spawn("entities", responder);
        let mut cfg = Config::default();
        cfg.daemon.pipe_name = mock.pipe_name.clone();
        let code = tokio::time::timeout(Duration::from_secs(5), run(args, &cfg, false))
            .await
            .expect("entities must return promptly");
        (code, mock.received())
    }

    #[tokio::test]
    async fn sends_list_all_entities() {
        let (code, reqs) = run_entities(
            EntitiesArgs {
                kind: None,
                action: None,
            },
            |_req| {
                Response::Ok(serde_json::json!([
                    { "kind": "person", "value": "Ada Lovelace", "count": 2 },
                    { "kind": "org", "value": "ACME Corp", "count": 1 },
                ]))
            },
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::ListAllEntities]);
    }

    #[tokio::test]
    async fn empty_facet_still_succeeds() {
        let (code, reqs) = run_entities(
            EntitiesArgs {
                kind: None,
                action: None,
            },
            |_req| Response::Ok(serde_json::json!([])),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::ListAllEntities]);
    }

    #[tokio::test]
    async fn add_subcommand_sends_add_entity() {
        let id = RecordingId::new();
        let (code, reqs) = run_entities(
            EntitiesArgs {
                kind: None,
                action: Some(EntitiesAction::Add {
                    id: id.to_string(),
                    kind: "person".into(),
                    value: "Ada Lovelace".into(),
                }),
            },
            |_req| Response::Ok(serde_json::Value::Null),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::AddEntity {
                id,
                kind: "person".into(),
                value: "Ada Lovelace".into(),
            }]
        );
    }

    #[tokio::test]
    async fn edit_keeps_unchanged_fields() {
        let id = RecordingId::new();
        let (code, reqs) = run_entities(
            EntitiesArgs {
                kind: None,
                action: Some(EntitiesAction::Edit {
                    id: id.to_string(),
                    kind: "org".into(),
                    value: "acme".into(),
                    to_kind: None,
                    to_value: Some("Acme Corp".into()),
                }),
            },
            |_req| Response::Ok(serde_json::Value::Null),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        // to_kind omitted → new_kind keeps the current kind.
        assert_eq!(
            reqs,
            vec![Request::UpdateEntity {
                id,
                kind: "org".into(),
                value: "acme".into(),
                new_kind: "org".into(),
                new_value: "Acme Corp".into(),
            }]
        );
    }

    #[tokio::test]
    async fn edit_with_no_change_fails_without_sending() {
        let id = RecordingId::new();
        let (code, reqs) = run_entities(
            EntitiesArgs {
                kind: None,
                action: Some(EntitiesAction::Edit {
                    id: id.to_string(),
                    kind: "org".into(),
                    value: "acme".into(),
                    to_kind: None,
                    to_value: None,
                }),
            },
            |_req| Response::Ok(serde_json::Value::Null),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
        assert!(reqs.is_empty(), "nothing-to-change must not hit the daemon");
    }

    #[tokio::test]
    async fn delete_subcommand_sends_delete_entity() {
        let id = RecordingId::new();
        let (code, reqs) = run_entities(
            EntitiesArgs {
                kind: None,
                action: Some(EntitiesAction::Delete {
                    id: id.to_string(),
                    kind: "topic".into(),
                    value: "roadmap".into(),
                }),
            },
            |_req| Response::Ok(serde_json::Value::Null),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::DeleteEntity {
                id,
                kind: "topic".into(),
                value: "roadmap".into(),
            }]
        );
    }

    #[tokio::test]
    async fn merge_subcommand_sends_merge_entities() {
        let (code, reqs) = run_entities(
            EntitiesArgs {
                kind: None,
                action: Some(EntitiesAction::Merge {
                    kind: "org".into(),
                    to_value: "Acme Corp".into(),
                    from_values: vec!["acme".into(), "ACME".into()],
                }),
            },
            |_req| Response::Ok(serde_json::Value::Null),
        )
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            reqs,
            vec![Request::MergeEntities {
                kind: "org".into(),
                from_values: vec!["acme".into(), "ACME".into()],
                to_value: "Acme Corp".into(),
            }]
        );
    }
}
