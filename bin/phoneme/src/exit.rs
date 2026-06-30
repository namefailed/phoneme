//! Spec-defined CLI exit codes — the scriptable half of the CLI contract.
//!
//! Scripts branch on these, so they are stable API: 0 success, 1 generic
//! failure, 2 usage error (clap's own), 3 daemon not reachable, 4 whisper
//! backend unreachable/timed out, 5 hook failed, 6 invalid config, 7 not
//! found. [`from_ipc_kind`] is the single mapping from a daemon
//! [`IpcErrorKind`] to a code — every command's error path funnels through
//! it (via `Client::send`), so a given failure always exits the same way.

use phoneme_ipc::IpcErrorKind;

pub const SUCCESS: u8 = 0;
pub const GENERIC_FAIL: u8 = 1;
#[allow(dead_code)]
pub const USAGE_ERROR: u8 = 2;
pub const DAEMON_NOT_REACHABLE: u8 = 3;
pub const WHISPER_UNREACHABLE: u8 = 4;
pub const HOOK_FAILED: u8 = 5;
pub const INVALID_CONFIG: u8 = 6;
pub const NOT_FOUND: u8 = 7;

pub fn from_ipc_kind(kind: IpcErrorKind) -> u8 {
    match kind {
        IpcErrorKind::DaemonNotRunning | IpcErrorKind::PipeInUse | IpcErrorKind::ShuttingDown => {
            DAEMON_NOT_REACHABLE
        }
        IpcErrorKind::WhisperUnreachable | IpcErrorKind::WhisperTimeout => WHISPER_UNREACHABLE,
        IpcErrorKind::HookFailed => HOOK_FAILED,
        IpcErrorKind::InvalidConfig => INVALID_CONFIG,
        IpcErrorKind::NotFound => NOT_FOUND,
        _ => GENERIC_FAIL,
    }
}

// silence dead-code warnings for codes that the early subcommands
// (Tasks 5-11) wire up; SUCCESS is the default and isn't referenced
// by from_ipc_kind.
#[allow(dead_code)]
const _UNUSED_SUCCESS: u8 = SUCCESS;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_maps_to_known_code() {
        assert_eq!(
            from_ipc_kind(IpcErrorKind::DaemonNotRunning),
            DAEMON_NOT_REACHABLE
        );
        // PipeInUse and ShuttingDown share the DAEMON_NOT_REACHABLE arm with
        // DaemonNotRunning; pin them so a regression that drops one of them out
        // of the `|` pattern (falling through to GENERIC_FAIL) is caught — these
        // exit codes are the stable scriptable CLI contract.
        assert_eq!(from_ipc_kind(IpcErrorKind::PipeInUse), DAEMON_NOT_REACHABLE);
        assert_eq!(
            from_ipc_kind(IpcErrorKind::ShuttingDown),
            DAEMON_NOT_REACHABLE
        );
        assert_eq!(
            from_ipc_kind(IpcErrorKind::WhisperUnreachable),
            WHISPER_UNREACHABLE
        );
        // WhisperTimeout shares the WHISPER_UNREACHABLE arm with WhisperUnreachable.
        assert_eq!(
            from_ipc_kind(IpcErrorKind::WhisperTimeout),
            WHISPER_UNREACHABLE
        );
        assert_eq!(from_ipc_kind(IpcErrorKind::HookFailed), HOOK_FAILED);
        assert_eq!(from_ipc_kind(IpcErrorKind::InvalidConfig), INVALID_CONFIG);
        assert_eq!(from_ipc_kind(IpcErrorKind::NotFound), NOT_FOUND);
        assert_eq!(from_ipc_kind(IpcErrorKind::Internal), GENERIC_FAIL);
    }
}
