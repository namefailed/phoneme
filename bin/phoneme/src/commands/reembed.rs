//! `phoneme reembed` — clear every stored embedding and re-embed the whole
//! library with the currently-configured model.
//!
//! Maps 1:1 to the `ReembedAll` IPC request. The daemon returns immediately and
//! does the work in the background, so there is no progress to stream here — the
//! command just reports that the re-embed was kicked off. Use this after
//! changing the embedding model (a different model/dimension makes old vectors
//! unsearchable).

use crate::client::Client;
use phoneme_core::Config;
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(cfg: &Config, json: bool) -> ExitCode {
    let mut client = match Client::connect(cfg).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.send(Request::ReembedAll).await {
        Ok(value) => {
            if json {
                crate::output::print_json(&value);
            } else {
                println!("re-embed started in the background");
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
