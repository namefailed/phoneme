//! At-rest protection for secrets (API keys) in `config.toml`.
//!
//! This module encrypts API keys with the Windows Data Protection API (DPAPI,
//! `CryptProtectData`) before they touch disk, so they're never stored in the
//! clear. The encryption is keyed to the current user account, so only this user
//! on this machine can decrypt them. The ciphertext is hex-encoded and tagged
//! with [`PREFIX`] so reads can tell an encrypted value from a legacy plaintext
//! one.
//!
//! Backwards/forwards compatible by design:
//! - A legacy plaintext key (no prefix) reads back verbatim and gets re-encrypted
//!   on the next save, so migration is zero-touch.
//! - An empty key stays empty. We don't encrypt an empty value, which keeps
//!   "is a key set?" checks and a clean config working.
//! - Off Windows (CI, a non-Windows build) `protect` is a no-op passthrough so
//!   the crate still compiles and round-trips in tests; a `dpapi:` value that
//!   can't be decrypted there is treated as unset.
//!
//! This is the at-rest half of S-H2; the WebView-masking half lives in the tray's
//! `commands.rs`. The two compose: masking hides the (now-encrypted) key from the
//! renderer, and this keeps it off disk in the clear.

/// Marker prefixing a DPAPI-encrypted, hex-encoded secret on disk. Versioned so
/// the scheme can evolve without misreading old values.
const PREFIX: &str = "dpapi:v1:";

/// Set by [`protect`] whenever it has to write a key UNENCRYPTED because DPAPI
/// failed. `protect` runs deep inside serde serialization (see `config.rs`), so
/// it can't signal the caller directly — this process-global latch lets the
/// config-write path detect a fallback after the fact and surface it.
/// [`take_plaintext_fallback`] reads and clears it.
static PLAINTEXT_FALLBACK: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Read-and-clear the "a secret was written unencrypted" latch. The config-write
/// path calls this right after serializing so it can warn the user that at-rest
/// protection didn't hold for this save (a transient DPAPI failure). Returns
/// `true` exactly once per fallback burst.
pub fn take_plaintext_fallback() -> bool {
    PLAINTEXT_FALLBACK.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// Encrypt `plaintext` for storage. Empty stays empty; on Windows this returns
/// `dpapi:v1:<hex>`; off Windows (or if DPAPI fails) it returns the plaintext
/// unchanged (best-effort, with a warning) so a key is never lost.
pub fn protect(plaintext: &str) -> String {
    if plaintext.is_empty() {
        return String::new();
    }
    #[cfg(windows)]
    {
        if let Some(ciphertext) = dpapi_protect(plaintext.as_bytes()) {
            return format!("{PREFIX}{}", hex_encode(&ciphertext));
        }
        // At error level: this breaks the module's "never stored in the clear"
        // guarantee. We still return the plaintext rather than drop the key (a
        // transient DPAPI failure shouldn't silently lose a key the user typed),
        // but the failure must be visible, not buried in a warning. Latch it so
        // the config-write path can surface it after serialization too.
        PLAINTEXT_FALLBACK.store(true, std::sync::atomic::Ordering::Relaxed);
        tracing::error!(
            "DPAPI encrypt failed; the API key will be written to config.toml UNENCRYPTED as a fallback"
        );
    }
    plaintext.to_string()
}

/// Decrypt a stored value. A `dpapi:v1:` value is hex-decoded and decrypted; any
/// other value (legacy plaintext, or empty) is returned verbatim — that's the
/// migration path. If decryption fails (e.g. the config was copied from another
/// user/machine, or this isn't Windows), the key is treated as unset (empty) so
/// a garbage value never reaches a provider; the user simply re-enters it.
pub fn unprotect(stored: &str) -> String {
    let Some(hex) = stored.strip_prefix(PREFIX) else {
        return stored.to_string();
    };
    #[cfg(windows)]
    {
        match hex_decode(hex).and_then(|ct| dpapi_unprotect(&ct)) {
            Some(plaintext) => String::from_utf8_lossy(&plaintext).into_owned(),
            None => {
                tracing::warn!(
                    "DPAPI decrypt failed (config from another user/machine?); treating the API key as unset"
                );
                String::new()
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = hex; // a dpapi: blob is undecryptable off Windows
        String::new()
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(windows)]
fn dpapi_protect(plaintext: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: plaintext.len() as u32,
        pbData: plaintext.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: `in_blob` points at a live slice for the duration of the call;
    // DPAPI only reads it. On success it writes a LocalAlloc'd buffer into
    // `out_blob`, which we copy out and free immediately. UI_FORBIDDEN prevents
    // any interactive prompt (this runs headless in the daemon/tray).
    let ok = unsafe {
        CryptProtectData(
            &in_blob,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
    };
    if ok == 0 || out_blob.pbData.is_null() {
        return None;
    }
    // SAFETY: on success DPAPI guarantees `pbData`/`cbData` describe a valid buffer.
    let bytes =
        unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec() };
    // SAFETY: `pbData` was allocated by DPAPI via LocalAlloc; free it once.
    unsafe { LocalFree(out_blob.pbData as *mut core::ffi::c_void) };
    Some(bytes)
}

#[cfg(windows)]
fn dpapi_unprotect(ciphertext: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: ciphertext.len() as u32,
        pbData: ciphertext.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: `in_blob` points at a live slice DPAPI only reads; the optional
    // out-description pointer is null. On success `out_blob` gets a LocalAlloc'd
    // buffer we copy and free. Failure (wrong user/machine, corrupt data) returns
    // 0 and we report None.
    let ok = unsafe {
        CryptUnprotectData(
            &in_blob,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
    };
    if ok == 0 || out_blob.pbData.is_null() {
        return None;
    }
    // SAFETY: on success DPAPI guarantees `pbData`/`cbData` describe a valid buffer.
    let bytes =
        unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec() };
    // SAFETY: DPAPI-allocated; free once.
    unsafe { LocalFree(out_blob.pbData as *mut core::ffi::c_void) };
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stays_empty() {
        assert_eq!(protect(""), "");
        assert_eq!(unprotect(""), "");
    }

    #[test]
    fn legacy_plaintext_reads_back_verbatim() {
        // Migration: a key written before DPAPI (no prefix) has to round-trip on
        // read so existing configs keep working until the next save re-encrypts.
        assert_eq!(
            unprotect("sk-legacy-plaintext-key"),
            "sk-legacy-plaintext-key"
        );
    }

    #[test]
    fn hex_roundtrips_including_edge_bytes() {
        let bytes = [0u8, 1, 15, 16, 127, 128, 254, 255];
        assert_eq!(hex_decode(&hex_encode(&bytes)).unwrap(), bytes);
        assert!(hex_decode("xyz").is_none(), "non-hex rejected");
        assert!(hex_decode("abc").is_none(), "odd length rejected");
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_protect_unprotect_roundtrips() {
        let key = "sk-proj-abc123-secret";
        let stored = protect(key);
        assert!(
            stored.starts_with(PREFIX),
            "encrypted form is tagged: {stored}"
        );
        assert!(
            !stored.contains(key),
            "plaintext must not appear in the stored form"
        );
        assert_eq!(unprotect(&stored), key, "decrypt restores the original");
    }

    #[cfg(windows)]
    #[test]
    fn corrupt_dpapi_blob_reads_as_unset() {
        // A tampered/foreign ciphertext can't be decrypted → treated as unset,
        // never surfaced as a garbage key.
        assert_eq!(unprotect("dpapi:v1:deadbeef"), "");
    }

    #[test]
    fn take_plaintext_fallback_is_read_and_clear() {
        // The latch surfaces a DPAPI failure to the config-write path after the
        // fact. `protect`'s failure branch is Windows-only and not deterministically
        // triggerable, so drive the shared static directly to pin the swap-and-clear
        // contract on every platform: it must report a raised latch exactly once,
        // then read back cleared.
        //
        // Clear first so any leftover state from a real DPAPI fallback elsewhere in
        // the run doesn't poison this assertion. `protect` only ever *sets* the
        // latch (on failure), never clears it, so there is no racing writer that
        // could re-raise it between these calls.
        PLAINTEXT_FALLBACK.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(
            !take_plaintext_fallback(),
            "a cleared latch must read false"
        );

        // Simulate the failure branch latching the flag.
        PLAINTEXT_FALLBACK.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(
            take_plaintext_fallback(),
            "a raised latch must read true exactly once"
        );
        assert!(
            !take_plaintext_fallback(),
            "the latch must clear after one read (a second read is false)"
        );
    }
}
