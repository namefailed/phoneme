//! Foreground-window detection for per-app dictation overrides (Windows only).
//!
//! Dictation can deliver text differently depending on which app is focused
//! when you stop speaking — type into one app, paste into another, stay quiet
//! in a third. To do that the daemon has to know which app owns the foreground
//! window. [`foreground_app`] queries the focused window's process file stem
//! (lowercased, e.g. `"code"` for `Code.exe`) and its window title via Win32.
//!
//! Everything here is **best-effort**: an elevated foreground process, a
//! permission failure, or simply no focused window all yield `None` rather than
//! an error — a missing answer just means "fall back to the global behavior",
//! never a failed dictation. On non-Windows the whole module compiles to a stub
//! that returns `None`, so the daemon builds and runs identically everywhere.
//!
//! Privacy: the window title is potentially sensitive (it can hold a document
//! name, an email subject, a private chat partner). It is only ever READ when a
//! caller asks for it, only ever USED for the opt-in app-aware cleanup context,
//! and is never logged or persisted by this module.

/// A snapshot of the foreground (focused) window: which app owns it and what
/// its title bar reads. Returned by [`foreground_app`]; both fields are
/// best-effort and may be empty even when the snapshot itself succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundApp {
    /// Lowercased file stem of the foreground process executable
    /// (`"code"` for `C:\…\Code.exe`, `"chrome"`, `"explorer"`). Lowercased so
    /// it matches the per-app config map case-insensitively. Never empty when a
    /// snapshot is returned — a process whose name can't be read yields `None`
    /// from [`foreground_app`] instead.
    pub exe_name: String,
    /// The foreground window's title bar text, or an empty string when the
    /// window has no title. Potentially sensitive — see the module docs.
    pub window_title: String,
}

#[cfg(windows)]
mod imp {
    use super::ForegroundApp;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, MAX_PATH};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };

    /// Query the foreground window's process stem + title. Returns `None` when
    /// there is no foreground window, or the owning process can't be resolved
    /// to a name (elevation, exit, permission) — see the module docs.
    pub fn foreground_app() -> Option<ForegroundApp> {
        // SAFETY: GetForegroundWindow takes no arguments and returns a window
        // handle (or null when no window is focused); we null-check before use.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_null() {
            return None;
        }

        let exe_name = process_stem(hwnd)?;
        // A blank title is fine (some windows have none); only the process name
        // is required for a usable snapshot.
        let window_title = window_title(hwnd);
        Some(ForegroundApp {
            exe_name,
            window_title,
        })
    }

    /// The lowercased file stem of the process owning `hwnd`, or `None` when it
    /// can't be read. The process handle is always closed before returning.
    fn process_stem(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<String> {
        let mut pid: u32 = 0;
        // SAFETY: `hwnd` is a live window handle from GetForegroundWindow and
        // `pid` is a valid out-pointer for the duration of the call.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == 0 {
            return None;
        }

        // PROCESS_QUERY_LIMITED_INFORMATION is the least-privileged right that
        // still lets QueryFullProcessImageNameW read the image path, so this
        // works against more processes than the older full-query right would.
        // SAFETY: OpenProcess validates the requested rights against `pid`;
        // a failure (elevation, gone) returns a null handle, checked below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return None;
        }

        let mut buffer = vec![0u16; MAX_PATH as usize];
        let mut len = buffer.len() as u32;
        // SAFETY: `process` is a live handle with query rights; `buffer`/`len`
        // describe a writable u16 buffer and its capacity. The flags arg `0`
        // requests the win32 path form. `len` is updated to the chars written.
        let ok = unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut len) };
        // SAFETY: `process` was returned by OpenProcess and is closed exactly
        // once here, whether or not the query above succeeded.
        unsafe { CloseHandle(process) };

        if ok == 0 || len == 0 {
            return None;
        }
        buffer.truncate(len as usize);
        let path = OsString::from_wide(&buffer).to_string_lossy().into_owned();
        // Extract the file stem and lowercase it for case-insensitive matching
        // against the per-app config keys (the user types "Code.exe" or "code").
        std::path::Path::new(&path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|s| s.to_lowercase())
    }

    /// The title bar text of `hwnd`, or an empty string when it has none.
    fn window_title(hwnd: windows_sys::Win32::Foundation::HWND) -> String {
        let mut buffer = vec![0u16; 512];
        // SAFETY: `hwnd` is a live window handle; `buffer` is a writable u16
        // buffer of the stated length. The return is the count of chars copied
        // (excluding the NUL), 0 when there is no title.
        let len = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        if len <= 0 {
            return String::new();
        }
        buffer.truncate(len as usize);
        OsString::from_wide(&buffer).to_string_lossy().into_owned()
    }
}

#[cfg(not(windows))]
mod imp {
    use super::ForegroundApp;

    /// Non-Windows stub: foreground-window detection is Windows-only, so this
    /// always reports "no answer" and the daemon falls back to the global mode.
    pub fn foreground_app() -> Option<ForegroundApp> {
        None
    }
}

/// Snapshot the foreground (focused) window's owning app and title.
///
/// Returns `Some` with a lowercased process stem and the (possibly empty)
/// window title, or `None` when there's no focused window, the process can't be
/// resolved, or the platform isn't Windows. Best-effort and side-effect-free
/// (apart from the Win32 reads): callers treat `None` as "use the global
/// dictation behavior". See the module docs for the privacy stance on titles.
pub fn foreground_app() -> Option<ForegroundApp> {
    imp::foreground_app()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: querying the foreground window must never panic and must
    /// return a coherent shape — on a headless CI box or non-Windows it returns
    /// `None`; on a real desktop it returns a snapshot with a non-empty stem.
    #[test]
    fn foreground_query_is_panic_free_and_coherent() {
        if let Some(app) = foreground_app() {
            assert!(
                !app.exe_name.is_empty(),
                "a returned snapshot always has a process stem"
            );
            assert_eq!(
                app.exe_name,
                app.exe_name.to_lowercase(),
                "the process stem is lowercased for case-insensitive matching"
            );
        }
    }
}
