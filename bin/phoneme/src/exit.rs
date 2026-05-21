//! Spec-defined CLI exit codes.

use phoneme_ipc::IpcErrorKind;

pub const SUCCESS: u8 = 0;
pub const GENERIC_FAIL: u8 = 1;
#[allow(dead_code)]
pub const USAGE_ERROR: u8 = 2;
pub const DAEMON_NOT_REACHABLE: u8 = 3;
pub const LLM_UNREACHABLE: u8 = 4;
pub const HOOK_FAILED: u8 = 5;
pub const INVALID_CONFIG: u8 = 6;
pub const NOT_FOUND: u8 = 7;

pub fn from_ipc_kind(kind: IpcErrorKind) -> u8 {
    match kind {
        IpcErrorKind::DaemonNotRunning | IpcErrorKind::PipeInUse | IpcErrorKind::ShuttingDown => {
            DAEMON_NOT_REACHABLE
        }
        IpcErrorKind::LlmUnreachable | IpcErrorKind::LlmTimeout => LLM_UNREACHABLE,
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
        assert_eq!(from_ipc_kind(IpcErrorKind::LlmUnreachable), LLM_UNREACHABLE);
        assert_eq!(from_ipc_kind(IpcErrorKind::HookFailed), HOOK_FAILED);
        assert_eq!(from_ipc_kind(IpcErrorKind::InvalidConfig), INVALID_CONFIG);
        assert_eq!(from_ipc_kind(IpcErrorKind::NotFound), NOT_FOUND);
        assert_eq!(from_ipc_kind(IpcErrorKind::Internal), GENERIC_FAIL);
    }
}
