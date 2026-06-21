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

use crate::args::EntitiesArgs;
use crate::client::Client;
use crate::exit;
use crate::output;
use phoneme_core::{Config, EntityFacet};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: EntitiesArgs, cfg: &Config, json: bool) -> ExitCode {
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
    if let Some(kind) = args.kind.as_deref() {
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
        let (code, reqs) = run_entities(EntitiesArgs { kind: None }, |_req| {
            Response::Ok(serde_json::json!([
                { "kind": "person", "value": "Ada Lovelace", "count": 2 },
                { "kind": "org", "value": "ACME Corp", "count": 1 },
            ]))
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::ListAllEntities]);
    }

    #[tokio::test]
    async fn empty_facet_still_succeeds() {
        let (code, reqs) = run_entities(EntitiesArgs { kind: None }, |_req| {
            Response::Ok(serde_json::json!([]))
        })
        .await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(reqs, vec![Request::ListAllEntities]);
    }
}
