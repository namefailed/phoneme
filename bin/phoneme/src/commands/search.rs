use crate::args::SearchArgs;
use crate::client::Client;
use crate::output;
use phoneme_core::{Config, Recording};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: SearchArgs, cfg: &Config, json: bool) -> ExitCode {
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let value = match client
        .send(Request::SemanticSearch {
            query: args.query,
            limit: args.limit,
        })
        .await
    {
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
