//! The local transcription-model registry: which whisper.cpp GGML weights
//! Phoneme can download, their pinned URL + SHA-256, where they live on disk,
//! and how to enumerate / size / verify them. The single source of truth shared
//! by the CLI (`phoneme model`), the desktop model manager (the `wizard_*`
//! commands), and `doctor`, so the filename set — which is also the allow-list a
//! deletion or download must match, so neither can escape the models directory —
//! never drifts. The pinned hashes are kept in lock-step with
//! `src-tauri/src/checksums.rs` (a test there asserts it).

use std::path::{Path, PathBuf};

/// One whisper.cpp model Phoneme can download: its on-disk filename (the
/// download / delete / select key), the pinned download URL, and the SHA-256 its
/// bytes must hash to (lower-case hex).
#[derive(Debug, Clone, Copy)]
pub struct WhisperModel {
    /// On-disk filename — the download / delete / select key.
    pub file: &'static str,
    /// Pinned download URL (whisper.cpp GGML weights on Hugging Face).
    pub url: &'static str,
    /// The SHA-256 the downloaded bytes must hash to (lower-case hex).
    pub sha256: &'static str,
}

/// Every whisper.cpp model Phoneme knows how to download, ordered smallest →
/// largest. The single source of truth for the model dropdowns, the deletion +
/// download allow-list, and `doctor`'s storage report.
pub const WHISPER_MODELS: &[WhisperModel] = &[
    WhisperModel {
        file: "ggml-tiny.en.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin",
        sha256: "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f",
    },
    WhisperModel {
        file: "ggml-base.en.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
        sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
    },
    WhisperModel {
        file: "ggml-small.en.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin",
        sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
    },
    WhisperModel {
        file: "ggml-medium.en.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.en.bin",
        sha256: "cc37e93478338ec7700281a7ac30a10128929eb8f427dda2e865faa8f6da4356",
    },
    WhisperModel {
        file: "ggml-large-v3-turbo-q5_0.bin",
        url:
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
    },
    WhisperModel {
        file: "ggml-large-v3-turbo.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
    },
    WhisperModel {
        file: "ggml-large-v3.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
    },
];

/// The local app-data root that holds the catalog, queue, logs and downloaded
/// models. Honors the `PHONEME_DATA_LOCAL` override the daemon and integration
/// tests set, so every surface measures the directory that's actually written to.
pub fn data_local_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("PHONEME_DATA_LOCAL") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    directories::ProjectDirs::from("", "", "phoneme").map(|d| d.data_local_dir().to_path_buf())
}

/// The directory downloaded whisper models live in (`<data-local>/models`).
pub fn models_dir() -> Option<PathBuf> {
    data_local_dir().map(|d| d.join("models"))
}

/// A downloaded whisper model on disk: its filename, full path, and byte size.
#[derive(Debug, Clone)]
pub struct DownloadedModel {
    /// The model filename (e.g. `ggml-small.en.bin`).
    pub name: String,
    /// Full path to the file on disk.
    pub path: PathBuf,
    /// On-disk size in bytes.
    pub bytes: u64,
}

/// Enumerate the known whisper models actually present on disk, with their
/// sizes, ordered smallest → largest (the order of [`WHISPER_MODELS`]). Empty
/// when the models directory can't be resolved or holds none of them.
pub fn downloaded_models() -> Vec<DownloadedModel> {
    let Some(dir) = models_dir() else {
        return Vec::new();
    };
    WHISPER_MODELS
        .iter()
        .filter_map(|m| {
            let path = dir.join(m.file);
            let bytes = std::fs::metadata(&path)
                .ok()
                .filter(|md| md.is_file())?
                .len();
            Some(DownloadedModel {
                name: m.file.to_string(),
                path,
                bytes,
            })
        })
        .collect()
}

/// The registry entry for a model filename, if Phoneme manages it. `None` means
/// the name isn't a known model — the allow-list that keeps a download or delete
/// from ever targeting an arbitrary path (no entry holds a path separator).
pub fn whisper_model(name: &str) -> Option<&'static WhisperModel> {
    WHISPER_MODELS.iter().find(|m| m.file == name)
}

/// True if `name` is a whisper model filename Phoneme manages.
pub fn is_known_whisper_model(name: &str) -> bool {
    whisper_model(name).is_some()
}

/// Lower-case hex SHA-256 of the file at `path`, read in 1 MiB chunks so a 3 GB
/// model never sits in memory. Used to verify a freshly-downloaded model before
/// it's trusted, mirroring the desktop download's pinned-hash check.
pub fn sha256_hex(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        let _ = write!(s, "{b:02x}");
    }
    Ok(s)
}

/// Human-readable byte size, e.g. `465.0 MB` / `2.9 GB` (binary units).
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_model_allowlist_rejects_traversal_and_unknowns() {
        assert!(is_known_whisper_model("ggml-large-v3.bin"));
        assert!(is_known_whisper_model("ggml-tiny.en.bin"));
        // The whole point of the allow-list: nothing with a path separator, and
        // nothing off the list, can ever be a download/delete target.
        assert!(!is_known_whisper_model("../config.toml"));
        assert!(!is_known_whisper_model("ggml-large-v3.bin/../secret"));
        assert!(!is_known_whisper_model("catalog.db"));
        assert!(!is_known_whisper_model(""));
        for m in WHISPER_MODELS {
            assert!(
                !m.file.contains('/') && !m.file.contains('\\'),
                "{} has a separator",
                m.file
            );
            // Every entry must be a 64-hex SHA-256 and a pinned whisper.cpp URL.
            assert_eq!(m.sha256.len(), 64, "{} sha must be 64 hex chars", m.file);
            assert!(
                m.url.starts_with("https://"),
                "{} url must be https",
                m.file
            );
            assert!(
                m.url.ends_with(m.file),
                "{} url must end with its file",
                m.file
            );
        }
    }

    #[test]
    fn format_bytes_scales_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(465 * 1024 * 1024), "465.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn sha256_hex_known_vector() {
        // SHA-256("abc") = ba7816bf..., same constant used by src-tauri/src/checksums.rs.
        let dir = std::env::temp_dir();
        let p = dir.join("phoneme-test-sha256-abc.bin");
        std::fs::write(&p, b"abc").unwrap();
        let hex = sha256_hex(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
