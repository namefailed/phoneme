//! Connection to phoneme-daemon — the tray's single request/response pipe.
//!
//! [`Bridge`] wraps one `NamedPipeTransport` behind a mutex: every Tauri
//! command serializes through it (which is exactly why slow daemon work runs
//! detached on the daemon side — one stalled request would stall the whole
//! invoke surface). A failed *read-only* request triggers one transparent
//! reconnect-and-retry, so an established bridge self-heals across daemon
//! restarts without the WebView noticing. Mutating requests get the single
//! attempt only: the transport error can't tell "never executed" from
//! "executed, but the reply was lost when the pipe dropped", and silently
//! re-sending a non-idempotent mutation (`ImportRecording` mints a fresh
//! `RecordingId` per call) would duplicate it — see [`is_retry_safe`].
//!
//! [`BridgeSlot`] covers the other failure mode — never connected at all.
//! It is the lazily-reconnecting holder the rest of the tray actually talks
//! to: sync callers (hotkey handler, exit hook) `current()` a non-blocking
//! peek, async callers `get_or_connect()`, which re-runs auto-spawn +
//! connect under a write lock (concurrent callers reuse the winner's
//! connection) and caches the bridge for everyone. Event streaming does NOT
//! go through here — `events` opens its own dedicated subscription
//! connection, per the pipe protocol.

use phoneme_core::Config;
use phoneme_ipc::{NamedPipeTransport, Request, Response, Transport};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

/// First reconnect delay after a failed connect.
const BACKOFF_START: Duration = Duration::from_millis(250);

/// Ceiling the reconnect delay doubles up to and then holds at. We cap and keep
/// trying slowly rather than ever giving up: the daemon can be started long
/// after the tray, so a connect attempt must still fire once each window
/// elapses — there is deliberately no permanent "too many attempts" failure.
const BACKOFF_CAP: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Mutex<NamedPipeTransport>>,
    pipe_name: String,
    pub config: Arc<Config>,
}

impl Bridge {
    pub async fn connect(config: Config) -> anyhow::Result<Self> {
        let pipe_name = config.daemon.pipe_name.clone();
        let transport = NamedPipeTransport::connect(&pipe_name).await?;
        Ok(Self {
            inner: Arc::new(Mutex::new(transport)),
            pipe_name,
            config: Arc::new(config),
        })
    }

    pub async fn reconnect(&self) -> anyhow::Result<()> {
        let new_transport = NamedPipeTransport::connect(&self.pipe_name).await?;
        let mut guard = self.inner.lock().await;
        *guard = new_transport;
        Ok(())
    }

    pub async fn request(&self, req: Request) -> anyhow::Result<Response> {
        let mut guard = self.inner.lock().await;
        match guard.request(req.clone()).await {
            Ok(r) => Ok(r),
            Err(e) => {
                // Only read-only/idempotent requests are safe to silently
                // re-send: a transport Err can't distinguish "the daemon never
                // saw it" from "the daemon ran it but the reply was lost when
                // the pipe dropped". Re-sending a mutation in the latter case
                // double-executes it. For unsafe requests, surface the error
                // and let the caller decide.
                if !is_retry_safe(&req) {
                    return Err(e.into());
                }
                drop(guard);
                self.reconnect().await?;
                let mut guard = self.inner.lock().await;
                Ok(guard.request(req).await?)
            }
        }
    }
}

/// Rate-limiter state for the reconnect path, all decided against a passed-in
/// `Instant` so the gating logic is unit-testable with a fake clock and no real
/// IPC. Held behind a plain mutex inside [`BridgeSlot`]; every method is cheap
/// and synchronous, so it is never locked across an `.await`.
///
/// The policy is bounded exponential backoff with no hard limit: each failed
/// connect doubles the delay from [`BACKOFF_START`] up to [`BACKOFF_CAP`] and
/// then holds there, a successful connect resets it, and while inside a window
/// the slot reports the daemon as down without re-attempting the spawn+connect.
/// That stops a flurry of UI actions during an outage from spawn-storming the
/// daemon, while still healing on its own once the daemon comes up.
#[derive(Debug, Default)]
struct Backoff {
    /// Earliest instant a connect may be attempted again. `None` means no
    /// failure is currently being backed off — attempt immediately.
    next_attempt: Option<Instant>,
    /// The delay applied at the last failure; the next failure doubles it
    /// (saturating at [`BACKOFF_CAP`]). Zero until the first failure.
    current_delay: Duration,
}

impl Backoff {
    /// Whether a connect may be attempted at `now`. True when no backoff is
    /// active or the current window has elapsed; false while still inside it.
    fn may_attempt(&self, now: Instant) -> bool {
        match self.next_attempt {
            Some(t) => now >= t,
            None => true,
        }
    }

    /// Record a failed connect at `now`: start at [`BACKOFF_START`], then double
    /// each subsequent failure, saturating at [`BACKOFF_CAP`]. Arms the window
    /// that [`may_attempt`](Self::may_attempt) gates on.
    fn record_failure(&mut self, now: Instant) {
        let next = if self.current_delay.is_zero() {
            BACKOFF_START
        } else {
            (self.current_delay * 2).min(BACKOFF_CAP)
        };
        self.current_delay = next;
        self.next_attempt = Some(now + next);
    }

    /// Record a successful connect: clear the window and reset the delay, so the
    /// next outage starts backing off from [`BACKOFF_START`] again.
    fn record_success(&mut self) {
        self.next_attempt = None;
        self.current_delay = Duration::ZERO;
    }
}

/// Whether a failed [`Request`] may be silently reconnected-and-retried by
/// [`Bridge::request`].
///
/// `true` only for pure reads and genuinely idempotent operations — re-sending
/// one after a lost reply changes nothing. `false` for anything that
/// creates/mutates state, because the first attempt may already have executed
/// on the daemon before the connection dropped; a blind re-send would run it
/// twice (`ImportRecording` would duplicate the recording). The match is
/// exhaustive ON PURPOSE: a newly-added request variant fails to compile here
/// until its retry-safety is classified deliberately, rather than defaulting to
/// the dangerous side.
fn is_retry_safe(req: &Request) -> bool {
    use Request::*;
    match req {
        // ── Pure reads ───────────────────────────────────────────────────
        DaemonStatus
        | RecordStatus
        | ListRecordings { .. }
        | GetRecording { .. }
        | ListAiActivity { .. }
        | ListSavedSearches
        // Runs a stored saved search server-side: a pure list query, idempotent.
        | RunSavedSearch { .. }
        | RecognizeSpeakers { .. }
        | ListNamedVoices
        | ListMeeting { .. }
        | GetSegments { .. }
        | GetWords { .. }
        | GetOriginalTranscript { .. }
        | GetCleanTranscript { .. }
        | ListQueue
        | QueuePaused
        | QueueCounts
        | RunDoctor
        | ListTags
        | ListAllTags
        | TagsFor { .. }
        | TagUsageCounts
        | KindCounts
        | SemanticSearch { .. }
        | MoreLikeThis { .. } => true,

        // ── State-changing / non-idempotent — single attempt only ────────
        // Recording control: each toggles/creates recorder state.
        RecordStart { .. }
        | RecordStop
        | RecordToggle { .. }
        | RecordPause
        | RecordResume
        | RecordCancel
        | StartMeeting
        | StopMeeting
        | MeetingToggle
        // Library mutations — ImportRecording mints a fresh RecordingId per
        // call, so a re-send duplicates the recording (the motivating bug).
        | DeleteRecording { .. }
        // Deletes a whole meeting's tracks; a blind re-send after a lost reply
        // could race a partially-applied cascade, so single-attempt only.
        | DeleteSession { .. }
        // Destructive: clears the catalog and re-imports + re-enqueues every
        // recording. Never blind-retry.
        | RebuildCatalog
        | ImportRecording { .. }
        // Re-import inserts catalog rows + enqueues; a blind re-send could
        // double-enqueue freshly-relinked files, so single-attempt only.
        | ReimportFromDisk { .. }
        // Re-runs (re-enqueue work / fire hooks).
        | RetranscribeRecording { .. }
        | RefireHook { .. }
        | RerunCleanup { .. }
        | RerunSummary { .. }
        // Transcript & metadata edits.
        | UpdateTranscript { .. }
        // Find-replace mutates the transcript; classified single-attempt like
        // UpdateTranscript (a re-send after a lost reply could re-apply against
        // already-changed text), so never blind-retry.
        | FindReplace { .. }
        | UpdateMeetingName { .. }
        | UpdateNotes { .. }
        | SetFavorite { .. }
        | SetRecordingTitle { .. }
        // Tag suggestions.
        | SuggestTags { .. }
        | ApproveTagSuggestion { .. }
        | DismissTagSuggestion { .. }
        | ClearAllTagSuggestions
        // Pipeline / preview / speakers.
        | RestartWhisper
        | SkipCurrentStage
        | SetPreviewSource { .. }
        | SetSpeakerName { .. }
        // In-recording speaker correction (U1) — mutate segments + prose markers.
        | ReassignSegmentSpeaker { .. }
        | MergeSpeakers { .. }
        | SplitSpeaker { .. }
        // Queue management.
        | CancelQueued { .. }
        | ReorderQueue { .. }
        | SetQueuePaused { .. }
        | ClearFailed
        | DismissFailed { .. }
        | UpsertSavedSearch { .. }
        | DeleteSavedSearch { .. }
        | DismissSpeakerSuggestion { .. }
        | RenameNamedVoice { .. }
        | MergeNamedVoices { .. }
        | ForgetNamedVoice { .. }
        | UndoForgetNamedVoice { .. }
        | CancelAllQueued
        | CancelProcessing { .. }
        // Daemon lifecycle & config.
        | Shutdown
        | ReloadConfig
        | HookTest { .. }
        // Tag CRUD.
        | AddTag { .. }
        | UpdateTag { .. }
        | DeleteTag { .. }
        | AttachTag { .. }
        | DetachTag { .. }
        | MergeTags { .. }
        // Re-embed kicks off a background job.
        | ReembedAll
        // Subscription handshake — the bridge never sends this (events open
        // their own connection), and it returns no Response anyway.
        | SubscribeEvents => false,
    }
}

/// Shared, lazily-reconnecting holder for the daemon [`Bridge`].
///
/// The tray can launch before the daemon accepts connections (cold boot,
/// crash-restart): startup's connect then fails, and before this slot existed
/// the managed bridge stayed `None` for the tray's whole lifetime —
/// every command failed until an app restart, even though the startup log
/// promised "will retry on first action". The slot IS that retry: the first
/// caller that finds it empty re-runs the auto-spawn + connect and caches the
/// result for everyone. An ESTABLISHED bridge already self-heals per request
/// (see [`Bridge::request`]); the slot only covers the never-connected case.
///
/// Reconnect attempts are rate-limited by an internal [`Backoff`]: a failed
/// connect arms an exponential window, and while inside it `get_or_connect`
/// reports the daemon as down without re-running the spawn+connect, so a burst
/// of UI actions during an outage cannot spawn-storm the daemon. The cap holds
/// rather than gives up, so a daemon started later still heals — just on the
/// backoff cadence instead of instantly.
#[derive(Clone)]
pub struct BridgeSlot {
    inner: Arc<RwLock<Option<Bridge>>>,
    /// Reconnect rate-limiter, shared across clones of the slot.
    backoff: Arc<StdMutex<Backoff>>,
    /// False only in tests: a slot that never dials out, so unit tests can
    /// assert the disconnected error path without touching real pipes or
    /// spawning a real daemon.
    connect_enabled: bool,
}

impl BridgeSlot {
    pub fn new(initial: Option<Bridge>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(initial)),
            backoff: Arc::new(StdMutex::new(Backoff::default())),
            connect_enabled: true,
        }
    }

    /// A slot that never connects — for unit tests of the disconnected path.
    #[cfg(test)]
    pub fn offline() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            backoff: Arc::new(StdMutex::new(Backoff::default())),
            connect_enabled: false,
        }
    }

    /// Non-blocking peek for SYNC callers (the global-hotkey handler, the exit
    /// hook). `None` while disconnected — or while another task holds the
    /// write lock mid-connect, which those callers treat the same way.
    pub fn current(&self) -> Option<Bridge> {
        self.inner.try_read().ok().and_then(|g| g.clone())
    }

    /// The bridge, connecting first when the slot is empty (auto-spawning the
    /// daemon exactly like startup does). Concurrent callers serialize on the
    /// write lock; losers reuse the winner's connection instead of dialing
    /// their own.
    pub async fn get_or_connect(&self) -> Option<Bridge> {
        if let Some(b) = self.inner.read().await.clone() {
            return Some(b);
        }
        if !self.connect_enabled {
            return None;
        }
        let mut slot = self.inner.write().await;
        if let Some(b) = slot.clone() {
            return Some(b); // another caller connected while we waited
        }
        // Inside a backoff window: report down without re-attempting, so a burst
        // of UI actions during an outage cannot spawn-storm the daemon.
        if !self.backoff.lock().unwrap().may_attempt(Instant::now()) {
            return None;
        }
        let config = crate::config_io::read().unwrap_or_default();
        if let Err(e) = crate::auto_spawn::ensure_running(&config).await {
            tracing::warn!(error = %e, "could not auto-spawn daemon on retry");
        }
        match Bridge::connect(config).await {
            Ok(b) => {
                tracing::info!("connected to daemon on retry");
                self.backoff.lock().unwrap().record_success();
                *slot = Some(b.clone());
                Some(b)
            }
            Err(e) => {
                let mut backoff = self.backoff.lock().unwrap();
                backoff.record_failure(Instant::now());
                tracing::warn!(
                    error = %e,
                    retry_in_ms = backoff.current_delay.as_millis() as u64,
                    "daemon still unreachable; backing off"
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_backoff_attempts_immediately() {
        let now = Instant::now();
        let b = Backoff::default();
        assert!(b.may_attempt(now), "no failure recorded yet — attempt now");
    }

    #[test]
    fn first_failure_arms_the_start_window() {
        let now = Instant::now();
        let mut b = Backoff::default();
        b.record_failure(now);
        assert_eq!(b.current_delay, BACKOFF_START);
        // Still inside the window: gated.
        assert!(!b.may_attempt(now));
        assert!(!b.may_attempt(now + BACKOFF_START - Duration::from_millis(1)));
        // Window elapsed: free to attempt again.
        assert!(b.may_attempt(now + BACKOFF_START));
    }

    #[test]
    fn repeated_failures_double_up_to_the_cap() {
        let now = Instant::now();
        let mut b = Backoff::default();
        b.record_failure(now);
        assert_eq!(b.current_delay, BACKOFF_START); // 250ms
        b.record_failure(now);
        assert_eq!(b.current_delay, BACKOFF_START * 2); // 500ms
        b.record_failure(now);
        assert_eq!(b.current_delay, BACKOFF_START * 4); // 1s
                                                        // Drive well past the cap; it saturates and holds, never overflows.
        for _ in 0..20 {
            b.record_failure(now);
        }
        assert_eq!(b.current_delay, BACKOFF_CAP);
        b.record_failure(now);
        assert_eq!(b.current_delay, BACKOFF_CAP, "stays capped, no overshoot");
    }

    #[test]
    fn success_resets_the_backoff() {
        let now = Instant::now();
        let mut b = Backoff::default();
        b.record_failure(now);
        b.record_failure(now);
        b.record_success();
        assert_eq!(b.current_delay, Duration::ZERO);
        assert!(b.may_attempt(now), "reset slot attempts immediately again");
        // A fresh outage after recovery starts from the bottom, not the cap.
        b.record_failure(now);
        assert_eq!(b.current_delay, BACKOFF_START);
    }

    #[test]
    fn cap_holds_but_never_gives_up() {
        // The "limit" is cap-and-keep-trying-slowly: even after many failures,
        // once the (capped) window elapses an attempt is allowed again — the
        // daemon may be started long after the tray.
        let now = Instant::now();
        let mut b = Backoff::default();
        for _ in 0..50 {
            b.record_failure(now);
        }
        assert_eq!(b.current_delay, BACKOFF_CAP);
        assert!(!b.may_attempt(now + BACKOFF_CAP - Duration::from_millis(1)));
        assert!(b.may_attempt(now + BACKOFF_CAP));
    }

    /// The retry-safety classifier draws the line that prevents
    /// double-execution: reads may be silently re-sent after a dropped reply,
    /// mutations may not. `ImportRecording` is the motivating case — the daemon
    /// mints a fresh `RecordingId` per call, so a blind re-send would duplicate
    /// the imported recording.
    #[test]
    fn mutations_are_not_retried_but_reads_are() {
        // Read: safe to silently reconnect-and-resend.
        assert!(is_retry_safe(&Request::ListRecordings {
            filter: Default::default(),
        }));
        assert!(is_retry_safe(&Request::DaemonStatus));

        // Mutation: a re-send could double-execute it — single attempt only.
        assert!(!is_retry_safe(&Request::ImportRecording {
            path: "C:/audio/take1.wav".to_string(),
        }));
        // A few more non-idempotent guards so the boundary can't quietly drift.
        assert!(!is_retry_safe(&Request::RecordStart {
            mode: phoneme_core::RecordMode::Hold,
            in_place: false,
            recipe_id: None,
            whisper_model: None,
            source: None,
        }));
        assert!(!is_retry_safe(&Request::ReloadConfig));
        assert!(!is_retry_safe(&Request::DeleteRecording {
            id: phoneme_core::RecordingId::new(),
            keep_audio: false,
        }));
    }
}
