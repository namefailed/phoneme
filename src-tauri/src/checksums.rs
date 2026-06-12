//! Pinned SHA-256 checksums for every artifact the first-run wizard downloads
//! (S-H7).
//!
//! The download allow-list (`is_allowed_download_url`) already restricts *where*
//! bytes may come from, but an allowed host can still be compromised, MITM'd
//! behind a corporate proxy, or simply serve a corrupted file. So every artifact
//! Phoneme itself loads or extracts — the whisper GGML weights, the semantic
//! ONNX model + tokenizer, and the whisper-server release zip — is pinned to an
//! exact SHA-256 here. A download that doesn't match its pin is deleted and
//! rejected before the file is ever loaded, extracted, or marked complete.
//!
//! Adding a new download URL without adding its pin is a hard error at runtime
//! (`expected_sha256` returns `None`) and a compile-noticed gap in the tests
//! (`pinned_table_covers_every_wizard_url`): fail closed rather than run
//! un-verified bytes.
//!
//! ## Provenance of each hash (recorded 2026-06-12)
//! - whisper GGML `.bin` weights — the `lfs.oid` published by the Hugging Face
//!   API for `ggerganov/whisper.cpp` (`/api/models/ggerganov/whisper.cpp/tree/main`).
//!   For a Git-LFS file the `oid` *is* the SHA-256 of the file contents.
//! - semantic `model.onnx` — the `lfs.oid` from the Hugging Face API for
//!   `Xenova/all-MiniLM-L6-v2` (`/tree/main/onnx`).
//! - semantic `tokenizer.json` — NOT an LFS file, so HF publishes no content
//!   hash; self-computed from the file served at the pinned URL on 2026-06-12.
//! - `whisper-bin-x64.zip` — the `digest` field GitHub's release API reports for
//!   the asset on the version-locked `v1.8.4` tag, cross-checked against a
//!   self-computed SHA-256 of the downloaded artifact on 2026-06-12 (the two
//!   agree). The URL is version-locked, so the pin stays valid for that release.
//!
//! NOT pinned here, by design: the Ollama installer (`wizard_download_file` →
//! `https://ollama.com/download/OllamaSetup.exe`). It is an auto-updating,
//! third-party installer served from a floating "latest" URL with no published
//! per-version digest, and the user launches it themselves through the official
//! Ollama setup window. It stays gated by the host allow-list and the
//! temp-dir + `.exe`-only restriction in `wizard_run_installer`; pinning a
//! moving-target installer to a single hash would simply break on Ollama's next
//! release. If we ever ship our own runnable bytes from a fixed URL, pin them.

use sha2::{Digest, Sha256};
use std::path::Path;

/// One pinned artifact: the exact download URL and the SHA-256 its bytes must
/// hash to. The URL is matched in full so a model filename can never be swapped
/// for another repo's file behind the same host.
struct Pinned {
    url: &'static str,
    sha256: &'static str,
}

/// Every artifact the wizard downloads and Phoneme then loads/extracts, pinned
/// to its content SHA-256. Lower-cased hex, no `0x`/`sha256:` prefix.
///
/// Keep this in lock-step with the URLs the frontend passes to the download
/// commands (`FirstRunWizard`, `SectionWhisper`, `SectionPreview`) and the URLs
/// hard-coded in the download commands themselves. `pinned_table_covers_every_wizard_url`
/// fails if a wizard URL loses its pin here.
const PINNED: &[Pinned] = &[
    // ── whisper.cpp GGML weights (Hugging Face LFS oid) ───────────────────────
    Pinned {
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin",
        sha256: "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f",
    },
    Pinned {
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
        sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
    },
    Pinned {
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin",
        sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
    },
    Pinned {
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.en.bin",
        sha256: "cc37e93478338ec7700281a7ac30a10128929eb8f427dda2e865faa8f6da4356",
    },
    Pinned {
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
    },
    Pinned {
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
    },
    Pinned {
        url:
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
    },
    // ── semantic search model (all-MiniLM-L6-v2) ──────────────────────────────
    Pinned {
        // HF LFS oid.
        url: "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx",
        sha256: "759c3cd2b7fe7e93933ad23c4c9181b7396442a2ed746ec7c1d46192c469c46e",
    },
    Pinned {
        // Non-LFS; self-computed from the pinned URL on 2026-06-12.
        url: "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/tokenizer.json",
        sha256: "da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0",
    },
    // ── whisper-server release zip (verified BEFORE extraction) ───────────────
    Pinned {
        // GitHub release `digest` for the v1.8.4 asset, cross-checked self-computed.
        url: "https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.4/whisper-bin-x64.zip",
        sha256: "74f973345cb52ef5ba3ec9e7e7af8e48cc8c71722d1528603b80588a11f82e3e",
    },
];

/// The pinned SHA-256 for `url`, if it is a known wizard artifact.
///
/// `None` means the URL is not in the pin table. Callers MUST treat that as a
/// hard failure (fail closed): the host allow-list lets a whole domain through,
/// so an un-pinned URL on an allowed host is exactly the case we don't want to
/// run un-verified.
pub fn expected_sha256(url: &str) -> Option<&'static str> {
    PINNED.iter().find(|p| p.url == url).map(|p| p.sha256)
}

/// Compute the lower-case hex SHA-256 of a file on disk, reading it in chunks so
/// a multi-gigabyte model never has to sit in memory all at once.
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    // 1 MiB chunks — big enough to keep syscalls cheap on a 3 GB model, small
    // enough to stay off the stack/heap pressure radar.
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

/// Lower-case hex encoding, so the computed digest compares to the pinned
/// constants without pulling in a hex crate.
fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Verify a freshly-downloaded file at `path` against the pin for `url`, then
/// delete it and return an error if it doesn't match (or if `url` has no pin).
///
/// On success the file is left in place. On ANY failure — un-pinned URL, hash
/// mismatch, or an IO error while hashing — the file is removed so a corrupt or
/// tampered artifact is never loaded, extracted, or treated as a finished
/// download on the next run. The returned message is user-facing (the wizard
/// surfaces download errors verbatim), so it explains what to do.
pub fn verify_file_or_delete(path: &Path, url: &str) -> Result<(), String> {
    let Some(expected) = expected_sha256(url) else {
        let _ = std::fs::remove_file(path);
        return Err(format!(
            "This download isn't recognised by this version of Phoneme, so it \
             can't be verified and was discarded. Update Phoneme, or file an \
             issue if it persists. (unpinned URL: {url})"
        ));
    };
    verify_file_against(path, expected)
}

/// Hash `path` and delete + reject it unless it matches `expected` (lower-case
/// hex SHA-256). Split out from the URL lookup so the match/cleanup logic is
/// unit-testable against an arbitrary fixture without faking the pin table.
fn verify_file_against(path: &Path, expected: &str) -> Result<(), String> {
    let actual = match file_sha256(path) {
        Ok(h) => h,
        Err(e) => {
            let _ = std::fs::remove_file(path);
            return Err(format!("could not verify the downloaded file: {e}"));
        }
    };
    if !actual.eq_ignore_ascii_case(expected) {
        let _ = std::fs::remove_file(path);
        return Err(format!(
            "The downloaded file didn't match the expected fingerprint, so it \
             was discarded. Please retry — if it keeps happening the download \
             mirror may be compromised or this app needs an update. \
             (expected SHA-256 {expected}, got {actual})"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── file_sha256 against a known vector ────────────────────────────────────

    #[test]
    fn file_sha256_matches_known_vector() {
        // The canonical SHA-256 of "abc".
        const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("abc.txt");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(file_sha256(&f).unwrap(), ABC);
    }

    #[test]
    fn file_sha256_matches_empty_vector() {
        // SHA-256 of the empty input — exercises the zero-read loop exit.
        const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("empty");
        std::fs::write(&f, b"").unwrap();
        assert_eq!(file_sha256(&f).unwrap(), EMPTY);
    }

    // ── table lookup hit / miss ───────────────────────────────────────────────

    #[test]
    fn expected_sha256_hits_a_pinned_url() {
        let got = expected_sha256(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin",
        );
        assert_eq!(
            got,
            Some("921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f")
        );
    }

    #[test]
    fn expected_sha256_misses_unknown_url() {
        // An allowed host, but not a pinned artifact → no hash (must fail closed).
        assert_eq!(
            expected_sha256(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-nope.bin"
            ),
            None
        );
        assert_eq!(expected_sha256("https://evil.com/model.bin"), None);
    }

    // ── verify_file_or_delete behaviour ───────────────────────────────────────

    #[test]
    fn verify_passes_and_keeps_matching_file() {
        // The success branch of the verifier: a fixture whose bytes hash to the
        // expected pin is accepted and left on disk untouched.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("abc.txt");
        std::fs::write(&f, b"abc").unwrap();
        let ok = verify_file_against(
            &f,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );
        assert!(ok.is_ok(), "a matching file verifies: {ok:?}");
        assert!(f.exists(), "a verified file stays on disk");
    }

    #[test]
    fn verify_deletes_temp_on_hash_mismatch() {
        // A pinned URL whose bytes don't match → file is removed and an error
        // naming expected vs got is returned.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("ggml-tiny.en.bin");
        std::fs::write(&f, b"not the real model").unwrap();
        let url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin";

        let err = verify_file_or_delete(&f, url).unwrap_err();
        assert!(!f.exists(), "a mismatching temp file must be deleted");
        assert!(
            err.contains("didn't match the expected fingerprint"),
            "error should explain the mismatch in plain language: {err}"
        );
        assert!(
            err.contains("921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f"),
            "error should name the expected SHA-256: {err}"
        );
    }

    #[test]
    fn verify_deletes_temp_on_unpinned_url() {
        // An allowed-host URL with no pin → fail closed, delete, and tell the
        // user to update / file an issue.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("mystery.bin");
        std::fs::write(&f, b"whatever").unwrap();

        let err = verify_file_or_delete(&f, "https://huggingface.co/unknown/file.bin").unwrap_err();
        assert!(!f.exists(), "an unpinned download must be deleted");
        assert!(
            err.contains("isn't recognised") && err.contains("Update Phoneme"),
            "error should fail closed with an update/issue hint: {err}"
        );
    }

    // ── completeness: every wizard URL is pinned ──────────────────────────────
    // If anyone adds a download the wizard offers without pinning it here, this
    // fails — the table is the single source of truth and must never fall behind
    // the URLs the frontend/commands actually fetch.

    #[test]
    fn pinned_table_covers_every_wizard_url() {
        // The whisper GGML weights all resolve from this one repo path; the
        // frontend builds each URL as base + filename.
        const HF_WHISPER_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";
        // The whisper GGML filenames the wizard + settings sections offer
        // (FirstRunWizard `_whisper_model_choice`, SectionWhisper MODELS,
        // SectionPreview PREVIEW_MODELS, curatedModels CURATED_LOCAL_WHISPER).
        let whisper_files = [
            "ggml-tiny.en.bin",
            "ggml-base.en.bin",
            "ggml-small.en.bin",
            "ggml-medium.en.bin",
            "ggml-large-v3.bin",
            "ggml-large-v3-turbo.bin",
            "ggml-large-v3-turbo-q5_0.bin",
        ];
        for f in whisper_files {
            let url = format!("{HF_WHISPER_BASE}{f}");
            assert!(
                expected_sha256(&url).is_some(),
                "wizard offers {f} but it is not pinned: {url}"
            );
        }

        // The hard-coded download URLs in the commands (semantic model files and
        // the whisper-server release zip).
        for url in [
            "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx",
            "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/tokenizer.json",
            "https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.4/whisper-bin-x64.zip",
        ] {
            assert!(
                expected_sha256(url).is_some(),
                "a command downloads {url} but it is not pinned"
            );
        }
    }

    #[test]
    fn pinned_hashes_are_lowercase_hex_64() {
        // A typo'd pin (wrong length, upper-case, stray prefix) would never match
        // a computed digest and would brick that download. Catch it here.
        for p in PINNED {
            assert_eq!(p.sha256.len(), 64, "{} pin is not 64 hex chars", p.url);
            assert!(
                p.sha256
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{} pin must be lower-case hex",
                p.url
            );
        }
    }
}
