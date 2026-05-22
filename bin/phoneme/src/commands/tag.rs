use crate::args::{TagAction, TagArgs};
use crate::client;
use phoneme_core::Config;
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: TagArgs, cfg: &Config, is_json: bool) -> ExitCode {
    let mut conn = match client::Client::connect(cfg).await {
        Ok(c) => c,
        Err(e) => return e,
    };

    match args.action {
        TagAction::List => match conn.send(Request::ListTags).await {
            Ok(val) => {
                if is_json {
                    println!("{}", serde_json::to_string_pretty(&val).unwrap());
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
        },
        TagAction::Add { name, color } => match conn.send(Request::AddTag { name, color }).await {
            Ok(val) => {
                if is_json {
                    println!("{}", serde_json::to_string_pretty(&val).unwrap());
                } else {
                    println!("added tag");
                }
                ExitCode::SUCCESS
            }
            Err(e) => e,
        },
        TagAction::Delete { id } => match conn.send(Request::DeleteTag { id }).await {
            Ok(_) => ExitCode::SUCCESS,
            Err(e) => e,
        },
        TagAction::Attach { recording_id, tag } => {
            if let Some(id) = phoneme_core::id::RecordingId::parse(&recording_id) {
                match conn.send(Request::ListTags).await {
                    Ok(val) => {
                        let tags: Vec<phoneme_core::tags::Tag> =
                            serde_json::from_value(val).unwrap_or_default();
                        if let Some(t) = tags.into_iter().find(|t| t.name == tag) {
                            match conn
                                .send(Request::AttachTag {
                                    recording_id: id,
                                    tag_id: t.id,
                                })
                                .await
                            {
                                Ok(_) => ExitCode::SUCCESS,
                                Err(e) => e,
                            }
                        } else {
                            eprintln!("error: tag '{}' not found", tag);
                            ExitCode::FAILURE
                        }
                    }
                    Err(e) => e,
                }
            } else {
                eprintln!("error: invalid recording ID '{}'", recording_id);
                ExitCode::FAILURE
            }
        }
        TagAction::Detach { recording_id, tag } => {
            if let Some(id) = phoneme_core::id::RecordingId::parse(&recording_id) {
                match conn.send(Request::ListTags).await {
                    Ok(val) => {
                        let tags: Vec<phoneme_core::tags::Tag> =
                            serde_json::from_value(val).unwrap_or_default();
                        if let Some(t) = tags.into_iter().find(|t| t.name == tag) {
                            match conn
                                .send(Request::DetachTag {
                                    recording_id: id,
                                    tag_id: t.id,
                                })
                                .await
                            {
                                Ok(_) => ExitCode::SUCCESS,
                                Err(e) => e,
                            }
                        } else {
                            eprintln!("error: tag '{}' not found", tag);
                            ExitCode::FAILURE
                        }
                    }
                    Err(e) => e,
                }
            } else {
                eprintln!("error: invalid recording ID '{}'", recording_id);
                ExitCode::FAILURE
            }
        }
    }
}
