//! `phoneme ask "<QUESTION>"` — answer a question from your own transcripts,
//! grounded with citations (local RAG).
//!
//! Spawning path. The daemon embeds the question, retrieves the top grounding
//! chunks via the same hybrid (vector + FTS5/RRF) retriever as `phoneme search`,
//! builds a citation-instructed prompt, and streams the answer through the
//! configured `[llm_post_process]` provider — so it needs a running daemon (and
//! auto-spawns one).
//!
//! Because [`Request::Ask`] ACKs `ok_null()` immediately and streams the work
//! over [`DaemonEvent::AskActivity`], this command uses the two-connection
//! subscribe-then-send pattern (like `phoneme record`): subscribe on one
//! connection first — the daemon only delivers events to subscriptions that
//! exist at emit time — then send `Ask` on a second. The client mints the
//! `request_id`, so there is no subscribe/send race to lose; the consumer
//! filters the shared event bus by it.
//!
//! Output: the numbered sources first (`[n] label  (relevance%)`), then the
//! answer streamed to stdout, ending on the terminal `done` marker. A terminal
//! `error` prints to stderr and yields a non-zero exit. `--json` collects the
//! whole stream into `{ "answer": "...", "sources": [...] }`.

use crate::args::AskArgs;
use crate::client::Client;
use crate::exit;
use crate::output;
use futures::StreamExt;
use phoneme_core::types::ListKind;
use phoneme_core::{Config, ListFilter, RecordingId, RecordingStatus};
use phoneme_ipc::{AskSource, DaemonEvent, Request};
use std::io::Write;
use std::process::ExitCode;

pub async fn run(args: AskArgs, cfg: &Config, json: bool) -> ExitCode {
    // Subscribe first on its own connection: the daemon only delivers events to
    // subscriptions that exist when an event is emitted, and Ask starts
    // streaming as soon as it's sent. Subscribing consumes this connection's
    // request channel, so the `Ask` itself rides a second connection.
    let mut sub_client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let mut events = match sub_client.subscribe().await {
        Ok(s) => s,
        Err(code) => return code,
    };
    let mut control = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };

    // Build the optional Library scope from --tag/--status/--kind (None when no
    // scope flag is given, matching `phoneme search`).
    let filter = match build_scope(&mut control, &args).await {
        Ok(f) => f,
        Err(code) => return code,
    };

    // Client-minted correlation id. RecordingId::new() is a process-wide unique,
    // chronologically-sortable token — enough to filter this Ask's events on the
    // shared bus without pulling in a UUID dependency.
    let request_id = format!("ask-{}", RecordingId::new().as_str());

    if let Err(code) = control
        .send(Request::Ask {
            request_id: request_id.clone(),
            query: args.query.clone(),
            top_k: args.top_k,
            filter,
        })
        .await
    {
        return code;
    }

    // Consume the stream, keeping only this request's AskActivity events.
    let mut sources: Vec<AskSource> = Vec::new();
    let mut answer = String::new();
    let stdout = std::io::stdout();

    // Idle cap: a slow-but-progressing local LLM (a few tokens/sec on a weak
    // box) must not be killed mid-answer, so we only give up when nothing has
    // arrived for this long — `last_activity` resets on every delta/sources
    // event. Matches the daemon's own idle-based streaming timeout.
    let idle_timeout =
        std::time::Duration::from_secs(cfg.llm_post_process.timeout_secs.max(60) + 120);
    // A far looser absolute ceiling still guards against a provider that dribbles
    // a byte forever without ever finishing.
    let hard_cap = std::time::Duration::from_secs(3600);
    let start = std::time::Instant::now();
    let mut last_activity = start;

    // The loop breaks with `(exit code, succeeded)`; `succeeded` drives the
    // trailing newline (ExitCode isn't PartialEq, so it can't be compared after).
    let (exit_code, succeeded): (ExitCode, bool) = loop {
        if last_activity.elapsed() >= idle_timeout || start.elapsed() >= hard_cap {
            eprintln!("timed out waiting for the answer");
            break (ExitCode::from(exit::GENERIC_FAIL), false);
        }
        match tokio::time::timeout(std::time::Duration::from_millis(500), events.next()).await {
            Ok(Some(Ok(DaemonEvent::AskActivity {
                request_id: rid,
                sources: ev_sources,
                delta,
                done,
                error,
            }))) => {
                if rid != request_id {
                    continue; // another Ask sharing the bus
                }
                // The first non-empty event carries the citation sources. Print
                // them once, before any answer text, in plain mode.
                if !ev_sources.is_empty() && sources.is_empty() {
                    last_activity = std::time::Instant::now();
                    sources = ev_sources;
                    if !json {
                        print_sources(&sources);
                    }
                }
                if !delta.is_empty() {
                    last_activity = std::time::Instant::now();
                    if json {
                        answer.push_str(&delta);
                    } else {
                        print!("{delta}");
                        let _ = stdout.lock().flush();
                        answer.push_str(&delta);
                    }
                }
                if done {
                    if !error.is_empty() {
                        if !json {
                            // Terminate the streamed line cleanly before the error.
                            println!();
                        }
                        eprintln!("error: {error}");
                        break (ExitCode::from(exit::GENERIC_FAIL), false);
                    }
                    break (ExitCode::SUCCESS, true);
                }
            }
            Ok(Some(Ok(_))) => continue, // unrelated event on the shared bus
            Ok(Some(Err(e))) => {
                eprintln!("event stream error: {e}");
                break (ExitCode::from(exit::DAEMON_NOT_REACHABLE), false);
            }
            Ok(None) => {
                eprintln!("error: daemon closed the event stream before the answer finished");
                break (ExitCode::from(exit::DAEMON_NOT_REACHABLE), false);
            }
            Err(_) => continue, // 500ms poll slice with no event; keep waiting
        }
    };

    if json {
        // The structured shape: the full answer plus the citation sources.
        output::print_json(&serde_json::json!({
            "answer": answer,
            "sources": sources,
        }));
    } else if succeeded {
        // End the streamed answer with a newline so the shell prompt returns on
        // its own line. The empty-retrieval "nothing matched" reply also streams
        // as a delta, so this trailing newline is always the right finish.
        println!();
    }

    exit_code
}

/// Print the numbered citation sources block (plain mode), before the answer.
fn print_sources(sources: &[AskSource]) {
    if sources.is_empty() {
        return;
    }
    println!("Sources:");
    for s in sources {
        println!(
            "  [{}] {}  ({:.0}%)",
            s.n,
            s.label,
            (s.relevance * 100.0).clamp(0.0, 100.0)
        );
    }
    println!();
}

/// Build the optional scope filter from `--tag` / `--status` / `--kind`.
/// Returns `Ok(None)` when no scope flag is set (whole-library answer). Mirrors
/// `phoneme search`'s `build_scope`: a `--tag` name is resolved against the
/// daemon's full tag list (so a zero-recording tag scopes to its empty set
/// rather than erroring).
async fn build_scope(client: &mut Client, args: &AskArgs) -> Result<Option<ListFilter>, ExitCode> {
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
