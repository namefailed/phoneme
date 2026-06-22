//! On-demand Ollama launch with an ownership ledger.
//!
//! Phoneme detects Ollama (wizard ping, Doctor) but historically never
//! started it — an LLM step against a local Ollama that wasn't running just
//! failed. This module launches `ollama serve` exactly when a step needs it,
//! under one hard rule: **an Ollama that was already running when the daemon
//! first probed it is never ours** — never killed, never restarted, never
//! assigned to a job, for the daemon's whole lifetime. Only a process this
//! module spawned is Owned, joined to the daemon's kill-on-close job, and
//! stopped at shutdown. That keeps a user's own startup Ollama untouchable
//! while a "Phoneme-only" Ollama comes and goes with the daemon.
//!
//! The decision is a tiny state machine ([`next_action`]) so the ownership
//! rules are unit-testable without spawning anything; the async glue around
//! it holds one mutex across probe → spawn, which is what makes concurrent
//! LLM steps single-flight (two simultaneous cleanups can't double-spawn).

use phoneme_core::config::LlmPostProcessConfig;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Child;

/// Where a default (empty `api_url`) Ollama config points.
const DEFAULT_OLLAMA_BASE: &str = "http://127.0.0.1:11434";
/// Per-request probe timeout. Generous enough for a busy box, short enough
/// that an LLM step never stalls long on a dead endpoint.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// Longest we wait for a freshly-spawned `ollama serve` to answer before
/// letting the LLM call proceed (and fail with its normal "couldn't reach"
/// error if the server still isn't up).
const READY_TIMEOUT: Duration = Duration::from_secs(15);
/// Poll interval while waiting for readiness.
const READY_POLL: Duration = Duration::from_millis(300);

/// What the daemon knows about the local Ollama process.
enum Ledger {
    /// Never probed — the state every daemon starts in.
    Unprobed,
    /// It was already running at first probe. Sticky for the daemon's
    /// lifetime: never spawned over, never killed, never job-assigned.
    NotOurs,
    /// We spawned it. The child handle (not a bare pid) is kept so the
    /// shutdown kill can only ever hit our own process — a recycled pid can
    /// never make us terminate something else. Boxed: a `Child` is large and
    /// the other variants carry nothing (clippy::large_enum_variant).
    Owned { child: Box<Child> },
}

/// [`Ledger`] flattened for the pure decision function: `Owned` splits on
/// whether the child is still alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LedgerKind {
    Unprobed,
    NotOurs,
    OwnedAlive,
    OwnedDead,
}

/// What `ensure_ready` should do, given the ledger and a fresh probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// The endpoint answers — nothing to manage.
    UseRunning,
    /// First probe found it already running: record NotOurs (sticky) and use it.
    MarkNotOurs,
    /// Down, and ours to start: spawn `ollama serve` and own the child.
    Spawn,
    /// Our child is alive but not answering yet (still loading): wait for it,
    /// never spawn a second one.
    AwaitOwned,
    /// Down, but NotOurs (sticky): hands off — the LLM call surfaces its
    /// normal unreachable error.
    LeaveAlone,
}

/// The ownership state machine. Every transition the launcher can take lives
/// here so the rules are testable without processes or sockets:
/// reachable-at-first-probe is NotOurs forever; NotOurs is never spawned over
/// or killed; an alive Owned child is never doubled; only a dead Owned child
/// is relaunched.
pub(crate) fn next_action(kind: LedgerKind, reachable: bool) -> Action {
    match (kind, reachable) {
        (LedgerKind::Unprobed, true) => Action::MarkNotOurs,
        (LedgerKind::Unprobed, false) => Action::Spawn,
        (LedgerKind::NotOurs, true) => Action::UseRunning,
        (LedgerKind::NotOurs, false) => Action::LeaveAlone,
        (LedgerKind::OwnedAlive, true) => Action::UseRunning,
        (LedgerKind::OwnedAlive, false) => Action::AwaitOwned,
        // Our child died. If something answers anyway (the user started their
        // own meanwhile), just use it — the dead handle is reaped at shutdown
        // and killing it is a no-op. If nothing answers, relaunch.
        (LedgerKind::OwnedDead, true) => Action::UseRunning,
        (LedgerKind::OwnedDead, false) => Action::Spawn,
    }
}

/// `true` when `host` (as it appears in a URL) is this machine. Only loopback
/// endpoints are ever auto-launched — a remote Ollama is someone else's.
fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // URL hosts wrap IPv6 in brackets; strip them before parsing.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// The base URL (`scheme://host:port`) to probe and launch for, or `None`
/// when this connection must never trigger a launch: a non-Ollama provider,
/// a remote host, or an unparseable URL. An empty `api_url` is the provider
/// default — the standard local Ollama.
pub(crate) fn spawn_target(provider: &str, api_url: &str) -> Option<String> {
    if !provider.trim().eq_ignore_ascii_case("ollama") {
        return None;
    }
    let url = api_url.trim();
    if url.is_empty() {
        return Some(DEFAULT_OLLAMA_BASE.to_string());
    }
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    if !is_loopback_host(host) {
        return None;
    }
    let port = parsed.port_or_known_default()?;
    Some(format!("{}://{}:{}", parsed.scheme(), host, port))
}

/// Launcher + ledger, shared process-wide via `AppState`. One instance, one
/// mutex: that mutex held across probe → spawn is the single-flight guarantee.
pub struct OllamaLauncher {
    ledger: tokio::sync::Mutex<Ledger>,
    http: reqwest::Client,
    /// The daemon's kill-on-close job. Only an Owned child is ever assigned,
    /// so an unclean daemon death reaps our Ollama and nothing else.
    #[cfg(windows)]
    job: Option<std::sync::Arc<phoneme_core::job::KillOnCloseJob>>,
}

impl OllamaLauncher {
    #[cfg(windows)]
    pub fn new(job: Option<std::sync::Arc<phoneme_core::job::KillOnCloseJob>>) -> Self {
        Self {
            ledger: tokio::sync::Mutex::new(Ledger::Unprobed),
            http: reqwest::Client::new(),
            job,
        }
    }

    #[cfg(not(windows))]
    pub fn new() -> Self {
        Self {
            ledger: tokio::sync::Mutex::new(Ledger::Unprobed),
            http: reqwest::Client::new(),
        }
    }

    /// Make sure the local Ollama is up if `llm_cfg` needs one and the knob
    /// allows launching. Best-effort and quiet: every failure path just
    /// returns, leaving the LLM call to produce its normal error. Safe to
    /// call concurrently — callers serialize on the ledger mutex.
    pub async fn ensure_ready(&self, llm_cfg: &LlmPostProcessConfig, log_dir: &Path) {
        if !llm_cfg.autostart_ollama {
            return;
        }
        let Some(base) = spawn_target(&llm_cfg.provider, &llm_cfg.api_url) else {
            return;
        };

        // Single-flight from here: probe + decision + spawn happen under one
        // lock, so a concurrent LLM step waits and then sees the result
        // (NotOurs, or an Owned child already starting) instead of racing.
        let mut ledger = self.ledger.lock().await;
        let reachable = self.probe(&base).await;
        let kind = match &mut *ledger {
            Ledger::Unprobed => LedgerKind::Unprobed,
            Ledger::NotOurs => LedgerKind::NotOurs,
            Ledger::Owned { child } => match child.try_wait() {
                Ok(Some(_)) => LedgerKind::OwnedDead,
                // Still running — or unknowable; assume alive rather than
                // ever risking a second spawn.
                _ => LedgerKind::OwnedAlive,
            },
        };
        // Whether we still need to poll for readiness after releasing the
        // lock. The decision (and any ledger mutation) happens under the lock;
        // the 15s readiness wait must not, or it serializes every concurrent
        // LLM step behind a cold start.
        let wait_after = match next_action(kind, reachable) {
            Action::UseRunning => false,
            Action::MarkNotOurs => {
                tracing::info!(
                    %base,
                    "found an already-running Ollama; it stays untouched for this daemon's lifetime"
                );
                *ledger = Ledger::NotOurs;
                false
            }
            Action::LeaveAlone => {
                tracing::debug!(
                    %base,
                    "Ollama endpoint is down but the process was never ours — not launching over it"
                );
                false
            }
            Action::AwaitOwned => true,
            Action::Spawn => {
                // Store the Owned child *before* dropping the lock so a
                // concurrent caller sees OwnedAlive (→ AwaitOwned) and never
                // double-spawns. Only the readiness poll moves out from under
                // the lock.
                if let Some(child) = self.spawn(&base, log_dir) {
                    *ledger = Ledger::Owned {
                        child: Box::new(child),
                    };
                    true
                } else {
                    false
                }
            }
        };
        // Single-flight is already secured (NotOurs recorded, or the Owned
        // child stored); release the lock before the bounded readiness wait.
        drop(ledger);
        if wait_after {
            self.wait_ready(&base).await;
        }
    }

    /// Stop the Ollama this daemon launched, if any. A NotOurs (or never
    /// probed) Ollama is left exactly as it was — this is the shutdown half
    /// of the ownership rule.
    pub async fn shutdown(&self) {
        let mut ledger = self.ledger.lock().await;
        if let Ledger::Owned { child } = &mut *ledger {
            if !matches!(child.try_wait(), Ok(Some(_))) {
                tracing::info!("stopping the Ollama this daemon launched");
                let _ = child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            }
        }
    }

    /// One bounded GET against the server root. Ollama answers any request to
    /// `/` with 200 "Ollama is running"; any HTTP response at all counts as
    /// reachable (the LLM call decides whether it's *usable*).
    async fn probe(&self, base: &str) -> bool {
        self.http
            .get(base)
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
            .is_ok()
    }

    /// Poll until the endpoint answers or `READY_TIMEOUT` passes, so the LLM
    /// step that triggered the launch runs against a listening server instead
    /// of failing instantly. On timeout the step proceeds anyway and surfaces
    /// the normal unreachable error.
    async fn wait_ready(&self, base: &str) {
        let deadline = std::time::Instant::now() + READY_TIMEOUT;
        loop {
            if self.probe(base).await {
                return;
            }
            if std::time::Instant::now() >= deadline {
                tracing::warn!(
                    %base,
                    "Ollama did not become ready in time; the LLM step will report unreachable"
                );
                return;
            }
            tokio::time::sleep(READY_POLL).await;
        }
    }

    /// Spawn `ollama serve` from PATH, windowless, with its output appended
    /// to `<log_dir>/ollama.log`. Returns `None` (logged) when the binary is
    /// missing or the spawn fails — the caller leaves the ledger unchanged so
    /// a later call retries (e.g. after the user installs Ollama).
    fn spawn(&self, base: &str, log_dir: &Path) -> Option<Child> {
        let exe = match which::which("ollama") {
            Ok(p) => p,
            Err(_) => {
                tracing::info!(
                    "an LLM step needs the local Ollama but no `ollama` binary is on PATH; \
                     install Ollama or set [llm_post_process] autostart_ollama = false"
                );
                return None;
            }
        };

        // `ollama serve` logs to stderr; capture both streams next to the
        // daemon's own logs so "why didn't my model load" is diagnosable.
        let log_path = log_dir.join("ollama.log");
        let (stdout, stderr) = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|f| {
                let clone = f.try_clone()?;
                Ok((f, clone))
            }) {
            Ok((out, err)) => (Stdio::from(out), Stdio::from(err)),
            Err(e) => {
                tracing::warn!(error = %e, path = %log_path.display(), "could not open ollama.log; discarding Ollama output");
                (Stdio::null(), Stdio::null())
            }
        };

        let mut cmd = tokio::process::Command::new(&exe);
        cmd.arg("serve")
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        match cmd.spawn() {
            Ok(child) => {
                tracing::info!(
                    pid = child.id().unwrap_or(0),
                    %base,
                    exe = %exe.display(),
                    "launched `ollama serve` (Owned — stopped again at daemon shutdown)"
                );
                // Owned children join the daemon's kill-on-close job so even
                // a task-manager-killed daemon can't leak this process. A
                // NotOurs Ollama never reaches this code path.
                #[cfg(windows)]
                if let Some(job) = &self.job {
                    match child.raw_handle() {
                        Some(h) => {
                            if let Err(e) = job.assign_raw(h) {
                                tracing::warn!(error = %e, "could not add Ollama to the daemon job; it may outlive an unclean daemon death");
                            }
                        }
                        None => {
                            tracing::warn!("Ollama child has no handle to job-assign; skipping")
                        }
                    }
                }
                Some(child)
            }
            Err(e) => {
                tracing::warn!(error = %e, exe = %exe.display(), "failed to launch `ollama serve`");
                None
            }
        }
    }

    /// Current ledger state, flattened — test hook for the ownership rules.
    #[cfg(test)]
    pub(crate) async fn kind_for_test(&self) -> LedgerKind {
        let mut ledger = self.ledger.lock().await;
        match &mut *ledger {
            Ledger::Unprobed => LedgerKind::Unprobed,
            Ledger::NotOurs => LedgerKind::NotOurs,
            Ledger::Owned { child } => match child.try_wait() {
                Ok(Some(_)) => LedgerKind::OwnedDead,
                _ => LedgerKind::OwnedAlive,
            },
        }
    }

    /// Force the ledger to Owned over a caller-spawned child — test hook for
    /// the shutdown/kill half without launching a real Ollama.
    #[cfg(test)]
    pub(crate) async fn set_owned_for_test(&self, child: Child) {
        *self.ledger.lock().await = Ledger::Owned {
            child: Box::new(child),
        };
    }

    /// Force the ledger to NotOurs — test hook for the sticky rule.
    #[cfg(test)]
    pub(crate) async fn set_not_ours_for_test(&self) {
        *self.ledger.lock().await = Ledger::NotOurs;
    }
}

/// Launch the local Ollama when the effective LLM connection needs one.
/// Call right before an LLM step actually runs; validation-only checks keep
/// calling `LlmPostProcessor::provider` directly so they never spawn anything.
pub async fn ensure_ready(state: &crate::app_state::AppState, llm_cfg: &LlmPostProcessConfig) {
    state
        .ollama
        .ensure_ready(llm_cfg, &state.paths.log_dir)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ownership state machine ─────────────────────────────────────────────

    #[test]
    fn first_probe_reachable_marks_not_ours() {
        // The user's own Ollama (running before we ever looked) must be
        // recorded as NotOurs — the root of the never-touch guarantee.
        assert_eq!(next_action(LedgerKind::Unprobed, true), Action::MarkNotOurs);
    }

    #[test]
    fn not_ours_is_sticky_never_spawned_over() {
        // Even if the user's Ollama later goes down, we never launch a
        // replacement (or anything else) — sticky for the daemon's lifetime.
        assert_eq!(next_action(LedgerKind::NotOurs, false), Action::LeaveAlone);
        assert_eq!(next_action(LedgerKind::NotOurs, true), Action::UseRunning);
    }

    #[test]
    fn unreachable_first_probe_spawns() {
        assert_eq!(next_action(LedgerKind::Unprobed, false), Action::Spawn);
    }

    #[test]
    fn alive_owned_child_is_never_doubled() {
        // A just-spawned server that hasn't bound its port yet must be waited
        // on, not raced with a second spawn.
        assert_eq!(
            next_action(LedgerKind::OwnedAlive, false),
            Action::AwaitOwned
        );
        assert_eq!(
            next_action(LedgerKind::OwnedAlive, true),
            Action::UseRunning
        );
    }

    #[test]
    fn dead_owned_child_relaunches_only_when_endpoint_is_down() {
        // Our child crashed: relaunch. But if something else answers now
        // (the user started their own), just use it — never kill it.
        assert_eq!(next_action(LedgerKind::OwnedDead, false), Action::Spawn);
        assert_eq!(next_action(LedgerKind::OwnedDead, true), Action::UseRunning);
    }

    // ── spawn-target gating ────────────────────────────────────────────────

    #[test]
    fn spawn_target_defaults_to_local_ollama() {
        assert_eq!(
            spawn_target("ollama", "").as_deref(),
            Some("http://127.0.0.1:11434")
        );
        assert_eq!(
            spawn_target("Ollama", "  ").as_deref(),
            Some("http://127.0.0.1:11434")
        );
    }

    #[test]
    fn spawn_target_accepts_loopback_urls_only() {
        // The full generate URL reduces to its server base.
        assert_eq!(
            spawn_target("ollama", "http://127.0.0.1:11434/api/generate").as_deref(),
            Some("http://127.0.0.1:11434")
        );
        assert_eq!(
            spawn_target("ollama", "http://localhost:11500").as_deref(),
            Some("http://localhost:11500")
        );
        assert_eq!(
            spawn_target("ollama", "http://[::1]:11434/api/generate").as_deref(),
            Some("http://[::1]:11434")
        );
        // Another machine's Ollama is never ours to start.
        assert_eq!(spawn_target("ollama", "http://192.168.1.50:11434"), None);
        assert_eq!(spawn_target("ollama", "https://ollama.example.com"), None);
        // Garbage URLs never trigger a launch.
        assert_eq!(spawn_target("ollama", "not a url"), None);
    }

    #[test]
    fn spawn_target_ignores_non_ollama_providers() {
        assert_eq!(spawn_target("openai", ""), None);
        assert_eq!(spawn_target("anthropic", "http://127.0.0.1:11434"), None);
        assert_eq!(spawn_target("none", ""), None);
        assert_eq!(spawn_target("", ""), None);
    }

    // ── async glue against a real (fake-HTTP) listener ─────────────────────

    /// A minimal HTTP responder on an ephemeral loopback port — enough for
    /// the probe's GET to succeed without an Ollama anywhere near the test.
    async fn fake_http_server() -> (tokio::net::TcpListener, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        (listener, base)
    }

    fn serve_one(listener: tokio::net::TcpListener) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .await;
            }
        })
    }

    fn local_cfg(api_url: &str, autostart: bool) -> LlmPostProcessConfig {
        let mut cfg = phoneme_core::config::Config::default().llm_post_process;
        cfg.enabled = true;
        cfg.provider = "ollama".into();
        cfg.api_url = api_url.into();
        cfg.autostart_ollama = autostart;
        cfg
    }

    #[cfg(windows)]
    fn launcher() -> OllamaLauncher {
        OllamaLauncher::new(None)
    }
    #[cfg(not(windows))]
    fn launcher() -> OllamaLauncher {
        OllamaLauncher::new()
    }

    /// An Ollama that was reachable at first probe becomes NotOurs — and a
    /// second call with the endpoint *down* stays NotOurs (sticky), spawning
    /// nothing.
    #[tokio::test]
    async fn reachable_endpoint_at_first_probe_is_not_ours_forever() {
        let (listener, base) = fake_http_server().await;
        let server = serve_one(listener);
        let launcher = launcher();
        let tmp = tempfile::tempdir().unwrap();

        launcher
            .ensure_ready(&local_cfg(&base, true), tmp.path())
            .await;
        assert_eq!(launcher.kind_for_test().await, LedgerKind::NotOurs);

        // Endpoint goes away; the sticky rule means we still launch nothing.
        server.abort();
        launcher
            .ensure_ready(&local_cfg(&base, true), tmp.path())
            .await;
        assert_eq!(
            launcher.kind_for_test().await,
            LedgerKind::NotOurs,
            "NotOurs must survive the endpoint going down"
        );
    }

    /// The knob gates everything: with `autostart_ollama = false` the
    /// launcher doesn't even probe, so the ledger never moves.
    #[tokio::test]
    async fn disabled_knob_probes_and_launches_nothing() {
        let (listener, base) = fake_http_server().await;
        let server = serve_one(listener);
        let launcher = launcher();
        let tmp = tempfile::tempdir().unwrap();

        launcher
            .ensure_ready(&local_cfg(&base, false), tmp.path())
            .await;
        assert_eq!(launcher.kind_for_test().await, LedgerKind::Unprobed);
        server.abort();
    }

    /// Concurrent calls serialize on the ledger: both see the same NotOurs
    /// outcome, no double work. (The mutex held across probe→spawn is the
    /// double-spawn guard; this exercises the contended path.)
    #[tokio::test]
    async fn concurrent_calls_share_one_ledger_outcome() {
        let (listener, base) = fake_http_server().await;
        let server = serve_one(listener);
        let launcher = std::sync::Arc::new(launcher());
        let tmp = tempfile::tempdir().unwrap();

        let cfg = local_cfg(&base, true);
        let (a, b) = tokio::join!(
            launcher.ensure_ready(&cfg, tmp.path()),
            launcher.ensure_ready(&cfg, tmp.path()),
        );
        let _ = (a, b);
        assert_eq!(launcher.kind_for_test().await, LedgerKind::NotOurs);
        server.abort();
    }

    /// Shutdown kills an Owned child (the daemon-launched Ollama) — proven
    /// with a stand-in child process, no real Ollama involved.
    #[cfg(windows)]
    #[tokio::test]
    async fn shutdown_kills_an_owned_child() {
        let launcher = launcher();
        let child = tokio::process::Command::new("cmd")
            .args(["/c", "ping -n 60 127.0.0.1 >NUL"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
            .expect("spawn stand-in child");
        launcher.set_owned_for_test(child).await;
        assert_eq!(launcher.kind_for_test().await, LedgerKind::OwnedAlive);

        launcher.shutdown().await;
        assert_eq!(
            launcher.kind_for_test().await,
            LedgerKind::OwnedDead,
            "the Owned child must be dead after shutdown"
        );
    }

    /// Shutdown with a NotOurs ledger touches nothing — the other half of
    /// the ownership rule. (Nothing to assert beyond "returns without
    /// killing"; the ledger stays NotOurs.)
    #[tokio::test]
    async fn shutdown_leaves_not_ours_alone() {
        let launcher = launcher();
        launcher.set_not_ours_for_test().await;
        launcher.shutdown().await;
        assert_eq!(launcher.kind_for_test().await, LedgerKind::NotOurs);
    }
}
