//! One module per `phoneme` subcommand. Each module's `run` takes its parsed
//! args + the loaded config, talks to the daemon through `crate::client`
//! (spawning vs observe-only per command — see the module docs), renders via
//! `crate::output`, and returns a `std::process::ExitCode` from
//! `crate::exit`'s table.

pub mod cleanup;
pub mod config_cmd;
pub mod daemon_cmd;
pub mod delete;
pub mod doctor;
pub mod edit;
pub mod export;
pub mod hook_cmd;
pub mod import;
pub mod list;
pub mod meeting;
pub mod notes;
pub mod profile_cmd;
pub mod queue;
pub mod record;
pub mod reembed;
pub mod refire_hook;
pub mod retranscribe;
pub mod search;
pub mod show;
pub mod speaker;
pub mod suggest_tags;
pub mod summarize;
pub mod tag;
pub mod watch;

#[cfg(test)]
pub mod test_support;
