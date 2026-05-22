use crate::args::{TagAction, TagArgs};
use crate::client;
use phoneme_core::{Config, RecordingId};
use phoneme_ipc::Request;
use std::process::ExitCode;

pub async fn run(args: TagArgs, cfg: &Config, is_json: bool) -> ExitCode {
    match args.action {
        TagAction::List => {
            let res = client::send_request(Request::ListTags, cfg).await;
            client::handle_json_response(res, is_json)
        }
        TagAction::Add { name, color } => {
            let res = client::send_request(Request::AddTag { name, color }, cfg).await;
            client::handle_json_response(res, is_json)
        }
        TagAction::Delete { id } => {
            let res = client::send_request(Request::DeleteTag { id }, cfg).await;
            client::handle_empty_response(res)
        }
        TagAction::Attach { recording_id, tag } => {
            if let Some(id) = phoneme_core::id::RecordingId::from_str_lenient(&recording_id) {
                // First, look up the tag ID by name
                let tags_res = client::send_request(Request::ListTags, cfg).await;
                match tags_res {
                    Ok(phoneme_ipc::Response::Ok(val)) => {
                        let tags: Vec<phoneme_core::tags::Tag> = serde_json::from_value(val).unwrap_or_default();
                        if let Some(t) = tags.into_iter().find(|t| t.name == tag) {
                            let res = client::send_request(Request::AttachTag { recording_id: id, tag_id: t.id }, cfg).await;
                            return client::handle_empty_response(res);
                        } else {
                            eprintln!("error: tag '{}' not found", tag);
                            return ExitCode::FAILURE;
                        }
                    }
                    _ => return client::handle_empty_response(tags_res),
                }
            } else {
                eprintln!("error: invalid recording ID '{}'", recording_id);
                ExitCode::FAILURE
            }
        }
        TagAction::Detach { recording_id, tag } => {
            if let Some(id) = phoneme_core::id::RecordingId::from_str_lenient(&recording_id) {
                // First, look up the tag ID by name
                let tags_res = client::send_request(Request::ListTags, cfg).await;
                match tags_res {
                    Ok(phoneme_ipc::Response::Ok(val)) => {
                        let tags: Vec<phoneme_core::tags::Tag> = serde_json::from_value(val).unwrap_or_default();
                        if let Some(t) = tags.into_iter().find(|t| t.name == tag) {
                            let res = client::send_request(Request::DetachTag { recording_id: id, tag_id: t.id }, cfg).await;
                            return client::handle_empty_response(res);
                        } else {
                            eprintln!("error: tag '{}' not found", tag);
                            return ExitCode::FAILURE;
                        }
                    }
                    _ => return client::handle_empty_response(tags_res),
                }
            } else {
                eprintln!("error: invalid recording ID '{}'", recording_id);
                ExitCode::FAILURE
            }
        }
    }
}
