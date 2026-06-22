//! Tauri commands — the WebView's entire `invoke("…")` surface.
//!
//! Three families live here:
//! - **Daemon forwards** — most commands are one `forward()` call: map the
//!   WebView's arguments onto a `phoneme_ipc::Request`, send it over the
//!   `BridgeSlot` (which lazily reconnects/auto-spawns when empty),
//!   and return the daemon's JSON value as-is. Errors come back as a
//!   structured [`CommandError`] `{kind, message}` whose `kind` is the
//!   IPC error kind's wire string, so the frontend branches the same way
//!   for every command. WebView-supplied recording ids are validated here
//!   (`parse_id`) before they can reach the daemon's fixed-offset
//!   accessors.
//! - **Local config/profile/window commands** — `read_config`/`write_config`
//!   and the profile commands work on config.toml directly, plus hotkey
//!   re-registration and window-state saving.
//! - **Wizard helpers** — checksum-pinned downloads, dependency detection,
//!   and connection tests (see `wizard` and `checksums`).
//!
//! ## Secret masking (S-H2)
//!
//! API keys never enter the WebView. `read_config` serializes the config
//! and replaces every non-empty `api_key` (whisper, llm_post_process,
//! summary, auto_tag, title, preview_whisper, and the nested
//! `in_place.stt`), plus the `webhook.hmac_secret` signing key, with the
//! `__phoneme_secret_kept__` sentinel. `write_config` restores any field
//! still holding the sentinel from the on-disk config before validating and
//! saving, so an unchanged key round-trips without ever leaving the Rust
//! side, and saving can never clobber a real key with the mask. The frontend
//! mirrors the sentinel constant. Commands that accept per-run key overrides
//! resolve the sentinel the same way (e.g. the cleanup re-run maps a masked
//! key back to the configured one).
//!
//! `write_config` also applies side effects after saving: registry Run-key
//! for start-at-login, `ReloadConfig` to the daemon, hotkey
//! re-registration, and overlay create/destroy.

use crate::bridge::BridgeSlot;
use crate::config_io;
use crate::doctor::CheckResult;
use crate::wizard::TestConnectResult;
use futures::StreamExt;
use phoneme_core::{Config, ListFilter, RecordMode, RecordingId, TranscriptSegment};
use phoneme_ipc::{Request, Response};
use serde_json::Value;
use tauri::{Emitter, State};

type Br<'r> = State<'r, BridgeSlot>;

mod config;
mod files;
mod recordings;
mod system;
mod wizard;

pub use config::*;
pub use files::*;
pub use recordings::*;
pub use system::*;
pub use wizard::*;

/// Structured error returned by Tauri commands. Serializes to `{ kind, message }`
/// so the WebView can branch on `kind` (e.g. tell `whisper_timeout` apart from
/// `not_found`) instead of parsing a flattened `"kind: message"` string.
///
/// `From<String>`/`From<&str>` map ad-hoc errors (config IO, validation) to a
/// generic `"error"` kind, so a command body's `?` on a `Result<_, String>`
/// helper still converts cleanly.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CommandError {
    pub kind: String,
    pub message: String,
}

impl CommandError {
    fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self {
            kind: "error".into(),
            message,
        }
    }
}

impl From<&str> for CommandError {
    fn from(message: &str) -> Self {
        Self {
            kind: "error".into(),
            message: message.into(),
        }
    }
}

async fn forward(slot: &BridgeSlot, req: Request) -> Result<Value, CommandError> {
    // An empty slot retries the connect (auto-spawning the daemon) before
    // giving up — the "down at launch" case heals on the first action instead
    // of requiring an app restart.
    let bridge = slot.get_or_connect().await.ok_or_else(|| {
        CommandError::new(
            "daemon_not_running",
            "daemon not reachable; start it with `phoneme daemon --start`",
        )
    })?;
    match bridge.request(req).await {
        Ok(Response::Ok(v)) => Ok(v),
        Ok(Response::Err(e)) => Err(CommandError::new(json_kind(&e.kind), e.message)),
        Err(e) => Err(CommandError::new(
            "transport",
            format!("transport error: {e}"),
        )),
    }
}

/// Validate a frontend-supplied recording id. A malformed id reaching the
/// daemon could panic in `RecordingId`'s fixed-offset slicing accessors, so
/// reject it here with a clean error instead.
fn parse_id(id: &str) -> Result<RecordingId, CommandError> {
    RecordingId::parse(id)
        .ok_or_else(|| CommandError::new("invalid_config", format!("invalid recording id: {id:?}")))
}

fn json_kind(k: &phoneme_ipc::IpcErrorKind) -> &'static str {
    use phoneme_ipc::IpcErrorKind::*;
    match k {
        AlreadyRecording => "already_recording",
        NotRecording => "not_recording",
        NotFound => "not_found",
        InvalidConfig => "invalid_config",
        WhisperUnreachable => "whisper_unreachable",
        WhisperTimeout => "whisper_timeout",
        HookFailed => "hook_failed",
        DaemonNotRunning => "daemon_not_running",
        PipeInUse => "pipe_in_use",
        ShuttingDown => "shutting_down",
        Io => "io",
        Internal => "internal",
    }
}

/// Placeholder the WebView sees in place of any saved API key, so secrets never
/// leave the daemon/tray process (S-H2). When the WebView writes config back, an
/// unchanged key arrives as this sentinel and we restore the real on-disk value
/// instead of clobbering it. The frontend mirrors this constant.
const MASKED_SECRET: &str = "__phoneme_secret_kept__";

/// Replace every non-empty API key in a serialized config with the mask.
fn mask_config_secrets(v: &mut Value) {
    // Driven by the single source of truth in phoneme-core so the GUI mask and
    // the CLI `phoneme config` redactor can't drift (e.g. webhook.custom_headers,
    // which one used to mask and the other leaked).
    phoneme_core::secrets::mask_json(v, MASKED_SECRET);
}

/// Restore any masked key in an incoming config from the current on-disk config,
/// so saving without changing a key keeps it rather than writing the placeholder.
fn unmask_config_secrets(incoming: &mut Config, current: &Config) {
    if incoming.whisper.api_key_str() == MASKED_SECRET {
        incoming
            .whisper
            .set_api_key(current.whisper.api_key_str().to_owned());
    }
    if incoming.llm_post_process.api_key_str() == MASKED_SECRET {
        incoming
            .llm_post_process
            .set_api_key(current.llm_post_process.api_key_str().to_owned());
    }
    if incoming.summary.api_key_str() == MASKED_SECRET {
        incoming
            .summary
            .set_api_key(current.summary.api_key_str().to_owned());
    }
    if incoming.auto_tag.api_key_str() == MASKED_SECRET {
        incoming
            .auto_tag
            .set_api_key(current.auto_tag.api_key_str().to_owned());
    }
    if incoming.title.api_key_str() == MASKED_SECRET {
        incoming
            .title
            .set_api_key(current.title.api_key_str().to_owned());
    }
    if let Some(pw) = incoming.preview_whisper.as_mut() {
        if pw.api_key_str() == MASKED_SECRET {
            let cur = current
                .preview_whisper
                .as_ref()
                .map(|c| c.api_key_str().to_owned())
                .unwrap_or_default();
            pw.set_api_key(cur);
        }
    }
    if let Some(stt) = incoming.in_place.stt.as_mut() {
        if stt.api_key_str() == MASKED_SECRET {
            let cur = current
                .in_place
                .stt
                .as_ref()
                .map(|c| c.api_key_str().to_owned())
                .unwrap_or_default();
            stt.set_api_key(cur);
        }
    }
    // Restore the webhook HMAC secret when it arrives still masked, so saving
    // config without touching it keeps the on-disk signing key.
    if incoming.webhook.hmac_secret_str() == MASKED_SECRET {
        incoming
            .webhook
            .set_hmac_secret(current.webhook.hmac_secret_str().to_owned());
    }
    // Webhook custom-header values arrive masked too; restore each still-masked
    // value from the on-disk config by key (drop the entry if the key no longer
    // exists on disk, so the placeholder is never persisted).
    let masked_headers: Vec<String> = incoming
        .webhook
        .custom_headers
        .iter()
        .filter(|(_, v)| v.as_str() == MASKED_SECRET)
        .map(|(k, _)| k.clone())
        .collect();
    for k in masked_headers {
        match current.webhook.custom_headers.get(&k) {
            Some(cur) => {
                incoming.webhook.custom_headers.insert(k, cur.clone());
            }
            None => {
                incoming.webhook.custom_headers.remove(&k);
            }
        }
    }
    // Playbook entries: restore each entry's masked llm.api_key from the on-disk
    // config by id, so saving without retyping a key keeps it.
    for entry in incoming.playbook.iter_mut() {
        if entry.llm.api_key_str() == MASKED_SECRET {
            let cur = current
                .playbook
                .iter()
                .find(|e| e.id == entry.id)
                .map(|e| e.llm.api_key_str().to_owned())
                .unwrap_or_default();
            entry.llm.set_api_key(cur);
        }
    }
}

/// True when `child`, once canonicalized, is `root` itself or lives under it.
/// Both paths are canonicalized so `..` traversal and symlinks can't escape the
/// allowed root. Returns `false` (fails closed) if either path can't be
/// canonicalized, e.g. it doesn't exist.
fn path_within(child: &std::path::Path, root: &std::path::Path) -> bool {
    match (std::fs::canonicalize(child), std::fs::canonicalize(root)) {
        (Ok(c), Ok(r)) => c.starts_with(&r),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn forward_disconnected_bridge_returns_descriptive_error() {
        // An offline slot never dials, so this exercises exactly the
        // "daemon unreachable and the retry failed" error path.
        let result = forward(&BridgeSlot::offline(), Request::DaemonStatus).await;
        let err = result.unwrap_err();
        assert_eq!(err.kind, "daemon_not_running");
        assert!(
            err.message.contains("daemon not reachable"),
            "expected daemon-not-reachable message, got: {err:?}"
        );
    }

    // ── config secret masking (S-H2) ──────────────────────────────────────

    #[test]
    fn mask_replaces_only_nonempty_keys() {
        let mut cfg = Config::default();
        cfg.llm_post_process.set_api_key("sk-secret-123");
        let mut json = serde_json::to_value(&cfg).unwrap();
        mask_config_secrets(&mut json);
        assert_eq!(json["llm_post_process"]["api_key"], MASKED_SECRET);
        // Whisper has no key by default — an empty key stays empty (not masked).
        assert_eq!(json["whisper"]["api_key"], "");
    }

    #[test]
    fn unmask_restores_unchanged_key_and_keeps_a_changed_one() {
        let mut current = Config::default();
        current.llm_post_process.set_api_key("real-cleanup-key");
        current.summary.set_api_key("real-summary-key");

        let mut incoming = current.clone();
        // Unchanged field arrives masked → restore from disk.
        incoming.llm_post_process.set_api_key(MASKED_SECRET);
        // Changed field carries the new key → keep it.
        incoming.summary.set_api_key("new-summary-key");

        unmask_config_secrets(&mut incoming, &current);
        assert_eq!(incoming.llm_post_process.api_key_str(), "real-cleanup-key");
        assert_eq!(incoming.summary.api_key_str(), "new-summary-key");
    }

    /// Completeness guard: mask and unmask are hand-enumerated, so a new
    /// secret-bearing field could be added to one but not the other (leaking a
    /// key to the WebView, or losing it on save). Set every secret to a unique
    /// sentinel, then assert (a) each is masked, (b) no plaintext sentinel
    /// survives anywhere in the JSON, and (c) unmask restores each, so the two
    /// functions can't silently drift out of sync.
    #[test]
    fn mask_unmask_cover_every_secret_field() {
        let mut cfg = Config::default();
        cfg.whisper.set_api_key("SECRET-whisper");
        cfg.llm_post_process.set_api_key("SECRET-cleanup");
        cfg.summary.set_api_key("SECRET-summary");
        cfg.auto_tag.set_api_key("SECRET-autotag");
        cfg.title.set_api_key("SECRET-title");
        let mut pw = cfg.whisper.clone();
        pw.set_api_key("SECRET-preview");
        cfg.preview_whisper = Some(pw);
        let mut stt = cfg.whisper.clone();
        stt.set_api_key("SECRET-dictation");
        cfg.in_place.stt = Some(stt);
        cfg.webhook.set_hmac_secret("SECRET-hmac");
        cfg.webhook
            .custom_headers
            .insert("Authorization".to_string(), "SECRET-header".to_string());
        // A built-in Playbook entry carries its own per-entry LLM key. Locate the
        // `cleanup` entry by id (not index) so the test stays correct if the seed
        // order ever changes, and record its array position for the assertions.
        let cleanup_idx = cfg
            .playbook
            .iter()
            .position(|e| e.id == "cleanup")
            .expect("the default config seeds a `cleanup` playbook entry");
        cfg.playbook[cleanup_idx].llm.set_api_key("SECRET-playbook");

        let mut json = serde_json::to_value(&cfg).unwrap();
        mask_config_secrets(&mut json);

        // (a) every enumerated secret now reads as the sentinel.
        assert_eq!(json["whisper"]["api_key"], MASKED_SECRET);
        assert_eq!(json["llm_post_process"]["api_key"], MASKED_SECRET);
        assert_eq!(json["summary"]["api_key"], MASKED_SECRET);
        assert_eq!(json["auto_tag"]["api_key"], MASKED_SECRET);
        assert_eq!(json["title"]["api_key"], MASKED_SECRET);
        assert_eq!(json["preview_whisper"]["api_key"], MASKED_SECRET);
        assert_eq!(json["in_place"]["stt"]["api_key"], MASKED_SECRET);
        assert_eq!(json["webhook"]["hmac_secret"], MASKED_SECRET);
        assert_eq!(
            json["webhook"]["custom_headers"]["Authorization"],
            MASKED_SECRET
        );
        assert_eq!(
            json["playbook"][cleanup_idx]["llm"]["api_key"],
            MASKED_SECRET
        );

        // (b) no plaintext sentinel survives anywhere — catches a future field
        // that is serialized in the clear but forgotten by mask_config_secrets.
        fn no_secret(v: &serde_json::Value) -> bool {
            match v {
                serde_json::Value::String(s) => !s.contains("SECRET-"),
                serde_json::Value::Array(a) => a.iter().all(no_secret),
                serde_json::Value::Object(o) => o.values().all(no_secret),
                _ => true,
            }
        }
        assert!(no_secret(&json), "a secret survived masking: {json}");

        // (c) unmask restores every one of them (no key lost on save).
        let current = cfg.clone();
        let mut incoming: Config = serde_json::from_value(json).unwrap();
        unmask_config_secrets(&mut incoming, &current);
        assert_eq!(incoming.whisper.api_key_str(), "SECRET-whisper");
        assert_eq!(incoming.llm_post_process.api_key_str(), "SECRET-cleanup");
        assert_eq!(incoming.summary.api_key_str(), "SECRET-summary");
        assert_eq!(incoming.auto_tag.api_key_str(), "SECRET-autotag");
        assert_eq!(incoming.title.api_key_str(), "SECRET-title");
        assert_eq!(
            incoming.preview_whisper.as_ref().unwrap().api_key_str(),
            "SECRET-preview"
        );
        assert_eq!(
            incoming.in_place.stt.as_ref().unwrap().api_key_str(),
            "SECRET-dictation"
        );
        assert_eq!(incoming.webhook.hmac_secret_str(), "SECRET-hmac");
        assert_eq!(
            incoming.webhook.custom_headers["Authorization"],
            "SECRET-header"
        );
        assert_eq!(
            incoming
                .playbook
                .iter()
                .find(|e| e.id == "cleanup")
                .expect("cleanup entry survives the round-trip")
                .llm
                .api_key_str(),
            "SECRET-playbook"
        );
    }

    // ── parse_id ──────────────────────────────────────────────────────────

    #[test]
    fn parse_id_accepts_valid_id() {
        assert!(parse_id("20260519T143500042").is_ok());
    }

    #[test]
    fn parse_id_rejects_garbage() {
        let err = parse_id("not-an-id").unwrap_err();
        assert!(err.message.contains("invalid recording id"));
    }

    #[test]
    fn parse_id_rejects_empty_string() {
        assert!(parse_id("").is_err());
    }

    // ── json_kind exhaustive ──────────────────────────────────────────────

    #[test]
    fn json_kind_covers_all_variants() {
        use phoneme_ipc::IpcErrorKind::*;
        // Ensure every variant maps to a non-empty kebab-case string.
        let all = [
            AlreadyRecording,
            NotRecording,
            NotFound,
            InvalidConfig,
            WhisperUnreachable,
            WhisperTimeout,
            HookFailed,
            DaemonNotRunning,
            PipeInUse,
            ShuttingDown,
            Io,
            Internal,
        ];
        for variant in &all {
            let s = json_kind(variant);
            assert!(!s.is_empty(), "json_kind returned empty for {variant:?}");
            assert!(
                s.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "json_kind should be snake_case, got {s:?}"
            );
        }
    }

    // ── path_within ───────────────────────────────────────
    // The reveal/open/run commands hand a renderer-supplied path to the OS, so
    // `path_within` is the gate that keeps those to an allowed root. It
    // canonicalizes both sides (so `..` and 8.3/junction tricks can't escape a
    // lexical prefix) and fails closed when either path can't be canonicalized.
    // These need real on-disk dirs because canonicalize touches the filesystem.

    #[test]
    fn path_within_accepts_child_and_root_itself() {
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("sub").join("deep");
        std::fs::create_dir_all(&sub).unwrap();
        let file = sub.join("clip.wav");
        std::fs::write(&file, b"x").unwrap();

        assert!(path_within(&file, root.path()), "a nested file is within");
        assert!(path_within(&sub, root.path()), "a nested dir is within");
        assert!(
            path_within(root.path(), root.path()),
            "the root is within itself"
        );
    }

    #[test]
    fn path_within_rejects_traversal_escape() {
        // `<root>/sub/../../outside` canonicalizes above the root, so it's
        // denied even though the lexical string starts under it.
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("root");
        let outside = base.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.txt");
        std::fs::write(&secret, b"x").unwrap();

        let escaping = root.join("..").join("outside").join("secret.txt");
        assert!(
            !path_within(&escaping, &root),
            "a ..-traversal that lands outside the root is denied"
        );
    }

    #[test]
    fn path_within_rejects_prefix_sibling() {
        // `C:\data2` must not count as inside `C:\data`: the canonical
        // starts_with compares whole path components, not raw string prefixes.
        let base = tempfile::tempdir().unwrap();
        let data = base.path().join("data");
        let data2 = base.path().join("data2");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&data2).unwrap();
        let file = data2.join("f.txt");
        std::fs::write(&file, b"x").unwrap();

        assert!(
            !path_within(&file, &data),
            "a sibling dir sharing a name prefix is not within"
        );
    }

    #[test]
    fn path_within_fails_closed_on_nonexistent() {
        // canonicalize fails for a path that doesn't exist → fail closed
        // (false), so a not-yet-created target can never slip through.
        let root = tempfile::tempdir().unwrap();
        let ghost = root.path().join("does-not-exist.txt");
        assert!(
            !path_within(&ghost, root.path()),
            "a non-existent child fails closed"
        );
        let ghost_root = root.path().join("missing-root");
        let real = root.path().join("real.txt");
        std::fs::write(&real, b"x").unwrap();
        assert!(
            !path_within(&real, &ghost_root),
            "a non-existent root fails closed"
        );
    }

    #[test]
    fn path_within_handles_mixed_separators() {
        // The renderer often sends forward slashes on Windows; canonicalize
        // normalizes separators, so a mixed-separator child still resolves
        // under the root.
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("a").join("b");
        std::fs::create_dir_all(&sub).unwrap();
        let file = sub.join("c.wav");
        std::fs::write(&file, b"x").unwrap();

        // Rebuild the child path with forward slashes from the root string.
        let mixed = format!("{}/a/b/c.wav", root.path().to_string_lossy());
        assert!(
            path_within(std::path::Path::new(&mixed), root.path()),
            "a forward-slash child of a backslash root is still within"
        );
    }
}
