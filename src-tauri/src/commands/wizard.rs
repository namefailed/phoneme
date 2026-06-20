//! (split from the former commands.rs god-file — see mod.rs)

use super::*;

/// Rewrite a local bundled-whisper probe URL to the port the daemon reports
/// the server is ACTUALLY listening on. The daemon treats the configured
/// `bundled_server_port` as a preference and falls back to a free port when
/// another app holds it; `daemon_status` publishes the live ports as
/// `whisper_preferred_port`/`whisper_effective_port` (and the
/// `preview_whisper_*` pair). `None` = leave the URL alone: it isn't the
/// shape the frontend builds for a local server (`http://127.0.0.1:<port>`),
/// it doesn't name a preferred port, or no differing effective port is live.
fn effective_local_whisper_url(url: &str, status: &Value) -> Option<String> {
    let rest = url.trim().strip_prefix("http://127.0.0.1:")?;
    let port: u16 = rest.strip_suffix('/').unwrap_or(rest).parse().ok()?;
    let pairs = [
        ("whisper_preferred_port", "whisper_effective_port"),
        (
            "preview_whisper_preferred_port",
            "preview_whisper_effective_port",
        ),
    ];
    for (preferred_key, effective_key) in pairs {
        let preferred = status.get(preferred_key).and_then(Value::as_u64);
        let effective = status.get(effective_key).and_then(Value::as_u64);
        if let (Some(p), Some(e)) = (preferred, effective) {
            if p == u64::from(port) && e != p && u16::try_from(e).is_ok() {
                return Some(format!("http://127.0.0.1:{e}"));
            }
        }
    }
    None
}

#[tauri::command]
pub async fn wizard_test_whisper(
    bridge: Br<'_>,
    url: String,
) -> Result<TestConnectResult, CommandError> {
    // For the local bundled server, probe the port it is ACTUALLY on — the
    // daemon falls back from the configured port when another app holds it.
    // `current()` only peeks at an existing connection (never spawns a
    // daemon), so a wizard run without one probes `url` exactly as before.
    let mut target = url;
    if target.starts_with("http://127.0.0.1:") {
        if let Some(b) = bridge.current() {
            if let Ok(Response::Ok(status)) = b.request(Request::DaemonStatus).await {
                if let Some(rewritten) = effective_local_whisper_url(&target, &status) {
                    target = rewritten;
                }
            }
        }
    }
    Ok(crate::wizard::test_whisper_endpoint(&target).await)
}

#[derive(serde::Serialize, Clone)]
struct DownloadProgress {
    downloaded: u64,
    total: Option<u64>,
}

#[tauri::command]
pub async fn wizard_download_model(
    window: tauri::Window,
    url: String,
    filename: String,
) -> Result<String, CommandError> {
    if filename.contains('/') || filename.contains('\\') || filename.is_empty() {
        return Err(CommandError::from("Invalid filename"));
    }
    // Same gate as wizard_download_file: a compromised WebView must
    // not be able to pull arbitrary bytes into the models dir.
    if !is_allowed_download_url(&url) {
        return Err(CommandError::from(
            "Download URL is not from an allowed host",
        ));
    }

    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| "could not resolve project directories".to_string())?;
    let models_dir = dirs.data_local_dir().join("models");
    tokio::fs::create_dir_all(&models_dir)
        .await
        .map_err(|e| format!("failed to create models dir: {}", e))?;

    let dest_path = models_dir.join(&filename);
    // A 0-byte file is a husk from a previously failed download, not a model —
    // fall through and re-download over it. A non-empty file only counts as
    // "already downloaded" once it passes its pinned checksum: an interrupted
    // run (or a tampered cache) can leave a non-zero but wrong file behind, and
    // it must not skip hashing. A failed check deletes the file (inside
    // verify_file_or_delete) and falls through to a clean re-download.
    if tokio::fs::metadata(&dest_path)
        .await
        .is_ok_and(|m| m.len() > 0)
    {
        let verify_path = dest_path.clone();
        let verify_url = url.clone();
        let cached_ok = tokio::task::spawn_blocking(move || {
            crate::checksums::verify_file_or_delete(&verify_path, &verify_url)
        })
        .await
        .map_err(|e| format!("spawn_blocking error: {}", e))?
        .is_ok();
        if cached_ok {
            // Emit a fake progress event so the UI knows it's 100%
            let _ = window.emit(
                "download_progress",
                DownloadProgress {
                    downloaded: 1,
                    total: Some(1),
                },
            );
            return Ok(dest_path.to_string_lossy().into_owned());
        }
    }

    let response = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("download failed with status: {}", response.status()).into());
    }

    // Create the destination only once the server said yes — creating it
    // first left a 0-byte file behind on request failure, and the
    // already-downloaded check then treated that husk as a finished
    // download forever.
    let mut file = tokio::fs::File::create(&dest_path)
        .await
        .map_err(|e| format!("failed to create file: {}", e))?;

    let total = response.content_length();
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                drop(file);
                let _ = tokio::fs::remove_file(&dest_path).await;
                return Err(format!("stream error: {}", e).into());
            }
        };
        if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
            drop(file);
            let _ = tokio::fs::remove_file(&dest_path).await;
            return Err(format!("write error: {}", e).into());
        }
        downloaded += chunk.len() as u64;

        let _ = window.emit("download_progress", DownloadProgress { downloaded, total });
    }

    // Flush before hashing so every downloaded byte is on disk, then verify the
    // finished file against its pin. A mismatch (or an unpinned URL) deletes the
    // file and fails — the model is never handed back to be loaded.
    file.sync_all()
        .await
        .map_err(|e| format!("failed to flush model file: {}", e))?;
    drop(file);
    let verify_path = dest_path.clone();
    tokio::task::spawn_blocking(move || {
        crate::checksums::verify_file_or_delete(&verify_path, &url)
    })
    .await
    .map_err(|e| format!("spawn_blocking error: {}", e))??;

    Ok(dest_path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn wizard_download_semantic_model(window: tauri::Window) -> Result<String, CommandError> {
    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| "could not resolve project directories".to_string())?;
    let semantic_dir = dirs.data_local_dir().join("models").join("semantic");
    tokio::fs::create_dir_all(&semantic_dir)
        .await
        .map_err(|e| format!("failed to create semantic model dir: {}", e))?;

    let files = [
        (
            "model.onnx",
            "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx",
        ),
        (
            "tokenizer.json",
            "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/tokenizer.json",
        ),
    ];

    for (filename, url) in files {
        let dest_path = semantic_dir.join(filename);
        // Treat a pre-existing file as done only if it passes its pin; a partial
        // or tampered cache otherwise re-downloads (verify deletes it first).
        if tokio::fs::metadata(&dest_path).await.is_ok() {
            let verify_path = dest_path.clone();
            let verify_url = url.to_string();
            let cached_ok = tokio::task::spawn_blocking(move || {
                crate::checksums::verify_file_or_delete(&verify_path, &verify_url)
            })
            .await
            .map_err(|e| format!("spawn_blocking error: {}", e))?
            .is_ok();
            if cached_ok {
                let _ = window.emit(
                    "semantic_download_progress",
                    DownloadProgress {
                        downloaded: 1,
                        total: Some(1),
                    },
                );
                continue;
            }
        }

        let response = reqwest::get(url)
            .await
            .map_err(|e| format!("request failed: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("download failed with status: {}", response.status()).into());
        }

        // Create only after a successful response (see wizard_download_model).
        let mut file = tokio::fs::File::create(&dest_path)
            .await
            .map_err(|e| format!("failed to create file: {}", e))?;

        let total = response.content_length();
        let mut downloaded: u64 = 0;
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    drop(file);
                    let _ = tokio::fs::remove_file(&dest_path).await;
                    return Err(format!("stream error: {}", e).into());
                }
            };
            if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
                drop(file);
                let _ = tokio::fs::remove_file(&dest_path).await;
                return Err(format!("write error: {}", e).into());
            }
            downloaded += chunk.len() as u64;

            let _ = window.emit(
                "semantic_download_progress",
                DownloadProgress { downloaded, total },
            );
        }

        // Verify each file against its pin before moving on; a bad file is
        // deleted and the whole download fails (the model loads both files).
        file.sync_all()
            .await
            .map_err(|e| format!("failed to flush {}: {}", filename, e))?;
        drop(file);
        let verify_path = dest_path.clone();
        let verify_url = url.to_string();
        tokio::task::spawn_blocking(move || {
            crate::checksums::verify_file_or_delete(&verify_path, &verify_url)
        })
        .await
        .map_err(|e| format!("spawn_blocking error: {}", e))??;
    }

    Ok(semantic_dir.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn wizard_download_diarization_model(window: tauri::Window) -> Result<(), CommandError> {
    // Diarization uses speakrs which downloads models automatically via hf-hub
    // Since hf-hub blocks, we run it in a blocking task.
    // The UI handles this as an indeterminate progress bar (total = null).

    let _ = window.emit(
        "diarization_download_progress",
        DownloadProgress {
            downloaded: 0,
            total: None,
        },
    );

    tokio::task::spawn_blocking(move || {
        // Just instantiating the pipeline triggers the download of the 500MB ONNX models to the hf cache
        let _pipeline =
            speakrs::OwnedDiarizationPipeline::from_pretrained(speakrs::ExecutionMode::Cpu)
                .map_err(|e| format!("failed to download diarization models: {}", e))?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("spawn_blocking error: {}", e))??;

    // Emit 100% completion so the wizard knows it's done
    let _ = window.emit(
        "diarization_download_progress",
        DownloadProgress {
            downloaded: 1,
            total: Some(1),
        },
    );

    Ok(())
}

#[derive(serde::Serialize)]
pub struct SystemInfo {
    pub ram_mb: u64,
    pub vram_mb: u64,
}

#[tauri::command]
pub fn wizard_get_system_info() -> SystemInfo {
    let mut sys = sysinfo::System::new_all();
    sys.refresh_memory();
    let ram_mb = sys.total_memory() / 1024 / 1024;

    let mut vram_mb = 0;
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-Command", "(Get-CimInstance Win32_VideoController | Measure-Object -Property AdapterRAM -Sum).Sum"])
           .creation_flags(CREATE_NO_WINDOW);

        if let Ok(output) = cmd.output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Ok(bytes) = stdout.trim().parse::<u64>() {
                    vram_mb = bytes / 1024 / 1024;
                }
            }
        }
    }

    SystemInfo { ram_mb, vram_mb }
}

#[tauri::command]
pub async fn wizard_list_downloaded_models() -> Result<Vec<String>, CommandError> {
    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| "could not resolve project directories".to_string())?;
    let models_dir = dirs.data_local_dir().join("models");
    let mut downloaded = Vec::new();
    let models = [
        "ggml-tiny.en.bin",
        "ggml-base.en.bin",
        "ggml-small.en.bin",
        "ggml-medium.en.bin",
        "ggml-large-v3.bin",
        "ggml-large-v3-turbo.bin",
    ];
    for model in models {
        let path = models_dir.join(model);
        if tokio::fs::metadata(&path).await.is_ok() {
            downloaded.push(path.to_string_lossy().into_owned());
        }
    }
    Ok(downloaded)
}

#[tauri::command]
pub async fn wizard_download_server(window: tauri::Window) -> Result<String, CommandError> {
    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| "could not resolve project directories".to_string())?;
    let bin_dir = dirs.data_local_dir().join("bin");
    tokio::fs::create_dir_all(&bin_dir)
        .await
        .map_err(|e| format!("failed to create bin dir: {}", e))?;

    let exe_path = bin_dir.join("whisper-server.exe");
    if tokio::fs::metadata(&exe_path).await.is_ok() {
        let _ = window.emit(
            "server_download_progress",
            DownloadProgress {
                downloaded: 1,
                total: Some(1),
            },
        );
        return Ok(exe_path.to_string_lossy().into_owned());
    }

    let url =
        "https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.4/whisper-bin-x64.zip";

    // Download into a temp file
    let temp_zip = bin_dir.join("whisper-temp.zip");
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("download failed with status: {}", response.status()).into());
    }

    // Create only after a successful response (see wizard_download_model).
    let mut file = tokio::fs::File::create(&temp_zip)
        .await
        .map_err(|e| format!("failed to create temp zip file: {}", e))?;

    let total = response.content_length();
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                drop(file);
                let _ = tokio::fs::remove_file(&temp_zip).await;
                return Err(format!("stream error: {}", e).into());
            }
        };
        if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
            drop(file);
            let _ = tokio::fs::remove_file(&temp_zip).await;
            return Err(format!("write error: {}", e).into());
        }
        downloaded += chunk.len() as u64;

        let _ = window.emit(
            "server_download_progress",
            DownloadProgress { downloaded, total },
        );
    }

    // Explicitly sync and drop to ensure file is completely written before unzip
    if let Err(e) = file.sync_all().await {
        let _ = tokio::fs::remove_file(&temp_zip).await;
        return Err(format!("failed to flush zip file: {}", e).into());
    }
    drop(file);

    // Verify the zip against its pin BEFORE extracting (S-H7): we are about to
    // write executables out of this archive, so a mismatched or unpinned zip is
    // deleted and rejected here rather than unpacked. The pin is keyed on the
    // version-locked release URL above.
    let verify_zip = temp_zip.clone();
    let verify_url = url.to_string();
    tokio::task::spawn_blocking(move || {
        crate::checksums::verify_file_or_delete(&verify_zip, &verify_url)
    })
    .await
    .map_err(|e| format!("spawn_blocking error: {}", e))??;

    let zip_path = temp_zip.clone();
    let bin_path = bin_dir.clone();

    tokio::task::spawn_blocking(move || -> Result<(), CommandError> {
        let zip_file = std::fs::File::open(&zip_path)
            .map_err(|e| format!("failed to open downloaded zip: {}", e))?;

        let mut archive = zip::ZipArchive::new(zip_file)
            .map_err(|e| format!("failed to read zip archive: {}", e))?;

        for i in 0..archive.len() {
            let mut file = match archive.by_index(i) {
                Ok(f) => f,
                Err(_) => continue,
            };

            let outpath = match file.enclosed_name() {
                Some(path) => path.to_owned(),
                None => continue,
            };

            if file.is_file() {
                if let Some(file_name) = outpath.file_name().and_then(|n| n.to_str()) {
                    if file_name.ends_with(".exe") || file_name.ends_with(".dll") {
                        // Extract to a sibling temp path first, then rename into
                        // place atomically — a crash or write error mid-copy
                        // leaves only the .tmp, so the final target is never a
                        // trusted-but-truncated binary on the next launch.
                        let extract_to = bin_path.join(file_name);
                        let tmp_path = bin_path.join(format!("{}.tmp", file_name));
                        // Remove any stale .tmp from a prior interrupted run.
                        let _ = std::fs::remove_file(&tmp_path);
                        let mut outfile = std::fs::File::create(&tmp_path).map_err(|e| {
                            format!("failed to create temp file for {}: {}", file_name, e)
                        })?;
                        if let Err(e) = std::io::copy(&mut file, &mut outfile) {
                            let _ = std::fs::remove_file(&tmp_path);
                            return Err(format!("failed to extract {}: {}", file_name, e).into());
                        }
                        // Flush before rename so all bytes are on disk.
                        if let Err(e) = outfile.sync_all() {
                            let _ = std::fs::remove_file(&tmp_path);
                            return Err(format!("failed to flush {}: {}", file_name, e).into());
                        }
                        drop(outfile);
                        std::fs::rename(&tmp_path, &extract_to).map_err(|e| {
                            let _ = std::fs::remove_file(&tmp_path);
                            format!("failed to place {}: {}", file_name, e)
                        })?;
                    }
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("spawn_blocking error: {}", e))??;

    let _ = tokio::fs::remove_file(&temp_zip).await;

    Ok(exe_path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn wizard_ping_ollama() -> Result<bool, CommandError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| CommandError::from(e.to_string()))?;
    match client
        .get("http://127.0.0.1:11434/api/version")
        .send()
        .await
    {
        Ok(r) => Ok(r.status().is_success()),
        Err(_) => Ok(false),
    }
}

#[tauri::command]
pub async fn wizard_detect_deps() -> Result<serde_json::Value, CommandError> {
    // Spawns the `ollama` CLI and stats the filesystem — both blocking — so run
    // it off the async runtime instead of holding a worker thread.
    tokio::task::spawn_blocking(|| -> Result<serde_json::Value, CommandError> {
        let mut has_ollama = false;

        // Check if `ollama` CLI is in PATH
        if let Ok(output) = std::process::Command::new("ollama")
            .arg("--version")
            .output()
        {
            if output.status.success() {
                has_ollama = true;
            }
        }

        // Check default Windows installation paths
        if !has_ollama {
            let localappdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
            if !localappdata.is_empty() {
                let ollama_path = std::path::Path::new(&localappdata)
                    .join("Programs")
                    .join("Ollama")
                    .join("ollama.exe");
                if ollama_path.exists() {
                    has_ollama = true;
                }
            }
        }

        if !has_ollama {
            let userprofile = std::env::var("USERPROFILE").unwrap_or_default();
            if !userprofile.is_empty() {
                let ollama_dir = std::path::Path::new(&userprofile).join(".ollama");
                if ollama_dir.exists() {
                    has_ollama = true;
                }
            }
        }

        Ok(serde_json::json!({
            "ollama": has_ollama,
        }))
    })
    .await
    .map_err(|e| CommandError::new("internal", format!("spawn_blocking error: {e}")))?
}

#[derive(serde::Serialize, Clone)]
pub struct OllamaPullProgress {
    pub status: String,
    pub completed: Option<u64>,
    pub total: Option<u64>,
}

#[tauri::command]
pub async fn wizard_pull_ollama_model(
    window: tauri::Window,
    model: String,
) -> Result<(), CommandError> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({ "name": model });
    let response = client
        .post("http://127.0.0.1:11434/api/pull")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("pull failed with status: {}", response.status()).into());
    }

    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream error: {}", e))?;
        if let Ok(s) = std::str::from_utf8(&chunk) {
            for line in s.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    let status = v["status"].as_str().unwrap_or("").to_string();
                    let completed = v["completed"].as_u64();
                    let total = v["total"].as_u64();
                    let _ = window.emit(
                        "ollama_pull_progress",
                        OllamaPullProgress {
                            status,
                            completed,
                            total,
                        },
                    );
                }
            }
        }
    }
    Ok(())
}

/// Hosts Phoneme may download from. Anything else is rejected so a compromised
/// renderer cannot fetch an arbitrary (e.g. malicious .exe) URL that could then
/// be run via wizard_run_installer.
fn is_allowed_download_url(url: &str) -> bool {
    if !url.starts_with("https://") {
        return false;
    }
    let host = match reqwest::Url::parse(url) {
        Ok(u) => match u.host_str() {
            Some(h) => h.to_ascii_lowercase(),
            None => return false,
        },
        Err(_) => return false,
    };
    const ALLOWED: &[&str] = &[
        "huggingface.co",
        "github.com",
        "objects.githubusercontent.com",
        "ollama.com",
        "registry.ollama.ai",
    ];
    ALLOWED
        .iter()
        .any(|a| host == *a || host.ends_with(&format!(".{a}")))
}

#[tauri::command]
pub async fn wizard_download_file(
    window: tauri::Window,
    url: String,
    filename: String,
) -> Result<String, CommandError> {
    if filename.contains('/') || filename.contains('\\') || filename.is_empty() {
        return Err(CommandError::from("Invalid filename"));
    }
    if !is_allowed_download_url(&url) {
        return Err(CommandError::from(
            "Download URL is not from an allowed host",
        ));
    }

    let dest_path = std::env::temp_dir().join(&filename);

    let response = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("download failed: {}", response.status()).into());
    }

    // Create only after a successful response (see wizard_download_model).
    let mut file = tokio::fs::File::create(&dest_path)
        .await
        .map_err(|e| format!("failed to create file: {}", e))?;

    let total = response.content_length();
    let mut downloaded: u64 = 0;

    use futures::StreamExt;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                drop(file);
                let _ = tokio::fs::remove_file(&dest_path).await;
                return Err(format!("stream error: {}", e).into());
            }
        };
        if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
            drop(file);
            let _ = tokio::fs::remove_file(&dest_path).await;
            return Err(format!("write error: {}", e).into());
        }
        downloaded += chunk.len() as u64;

        let _ = window.emit("download_progress", DownloadProgress { downloaded, total });
    }

    Ok(dest_path.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn wizard_run_installer(path: String) -> Result<(), CommandError> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(CommandError::from("Installer file does not exist"));
    }
    // Canonicalize BOTH sides before comparing: the old lexical
    // starts_with let "…\Temp\..\evil.exe" through (".." survives
    // Path::starts_with), and 8.3 short names / junctions could dodge a
    // prefix check entirely. path_within canonicalizes child and root.
    if !path_within(p, &std::env::temp_dir()) {
        return Err(CommandError::from(
            "Execution is restricted to the temporary directory",
        ));
    }
    // The wizard only ever downloads-and-runs .exe installers.
    let is_exe = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    if !is_exe {
        return Err(CommandError::from("Only .exe installers can be run"));
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new(&path)
            .spawn()
            .map_err(|e| format!("failed to run installer: {}", e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── effective_local_whisper_url ────────────────────────────────────────

    /// A daemon_status payload with the given port fields (null when None).
    fn status(
        preferred: Option<u64>,
        effective: Option<u64>,
        pv_preferred: Option<u64>,
        pv_effective: Option<u64>,
    ) -> Value {
        serde_json::json!({
            "running": true,
            "pid": 1,
            "whisper_preferred_port": preferred,
            "whisper_effective_port": effective,
            "preview_whisper_preferred_port": pv_preferred,
            "preview_whisper_effective_port": pv_effective,
        })
    }

    #[test]
    fn local_probe_url_follows_the_effective_port() {
        // The bundled server fell back from 5809 — the wizard's "Test" must
        // probe where it actually listens, with or without a trailing slash.
        let s = status(Some(5809), Some(51234), None, None);
        assert_eq!(
            effective_local_whisper_url("http://127.0.0.1:5809", &s).as_deref(),
            Some("http://127.0.0.1:51234")
        );
        assert_eq!(
            effective_local_whisper_url("http://127.0.0.1:5809/", &s).as_deref(),
            Some("http://127.0.0.1:51234")
        );
    }

    #[test]
    fn preview_probe_url_follows_the_preview_servers_port() {
        let s = status(Some(5809), Some(5809), Some(5810), Some(52345));
        assert_eq!(
            effective_local_whisper_url("http://127.0.0.1:5810", &s).as_deref(),
            Some("http://127.0.0.1:52345")
        );
    }

    #[test]
    fn non_matching_urls_are_left_alone() {
        let s = status(Some(5809), Some(51234), None, None);
        // External hosts, non-preferred local ports, and unparsable URLs are
        // never rewritten — only the configured bundled endpoint is ours.
        assert_eq!(
            effective_local_whisper_url("http://10.0.0.7:5809", &s),
            None
        );
        assert_eq!(
            effective_local_whisper_url("http://127.0.0.1:9000", &s),
            None
        );
        assert_eq!(effective_local_whisper_url("not a url", &s), None);
    }

    #[test]
    fn no_rewrite_when_effective_matches_or_is_missing() {
        // Server on its preferred port → nothing to fix.
        let same = status(Some(5809), Some(5809), None, None);
        assert_eq!(
            effective_local_whisper_url("http://127.0.0.1:5809", &same),
            None
        );
        // Server not running (effective null) → probe the configured URL so
        // the test fails with the honest "unreachable" the user should see.
        let down = status(Some(5809), None, None, None);
        assert_eq!(
            effective_local_whisper_url("http://127.0.0.1:5809", &down),
            None
        );
        // Older daemon without the fields at all → unchanged behavior.
        let old = serde_json::json!({ "running": true, "pid": 1 });
        assert_eq!(
            effective_local_whisper_url("http://127.0.0.1:5809", &old),
            None
        );
    }

    // ── is_allowed_download_url ────────────────────────────────────
    // The download allow-list is a security boundary: a compromised renderer
    // must not be able to point a download (whose bytes can later be run via
    // wizard_run_installer) at an arbitrary host. These pin the real contract:
    // https-only, host on the allow-list (exact or a true sub-domain), and the
    // classic spoofs (downgrade, look-alike, userinfo@, sub-domain suffix
    // confusion) all denied.

    #[test]
    fn allowed_urls_cover_the_real_wizard_hosts() {
        // Every host the wizard actually downloads from, as used in commands.rs
        // (model weights, the whisper-server zip, the semantic ONNX, Ollama).
        for url in [
            "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx",
            "https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.4/whisper-bin-x64.zip",
            "https://objects.githubusercontent.com/github-production-release-asset/x",
            "https://ollama.com/library/llama3",
            "https://registry.ollama.ai/v2/library/llama3/manifests/latest",
            // A genuine sub-domain of an allowed host is allowed (the `.{a}` arm).
            "https://cdn-lfs.huggingface.co/repos/abc/model.bin",
        ] {
            assert!(is_allowed_download_url(url), "should allow {url}");
        }
    }

    #[test]
    fn http_downgrade_is_denied() {
        // Plain http (or anything not https) is rejected outright, even for an
        // otherwise-allowed host — no MITM-able transport for runnable bytes.
        assert!(!is_allowed_download_url("http://huggingface.co/model.bin"));
        assert!(!is_allowed_download_url("ftp://github.com/x"));
        assert!(!is_allowed_download_url("HTTPS://github.com/x")); // scheme match is case-sensitive by design (starts_with "https://")
    }

    #[test]
    fn other_and_lookalike_hosts_are_denied() {
        for url in [
            "https://evil.com/payload.exe",
            // Look-alike: the allowed name as a sub-string but a different host.
            "https://huggingface.co.evil.com/model.bin",
            "https://githubXcom/x",
            "https://notgithub.com/x",
            // Allowed name only as a path/query component, not the host.
            "https://evil.com/huggingface.co/model.bin",
            "https://evil.com/?x=github.com",
        ] {
            assert!(!is_allowed_download_url(url), "should deny {url}");
        }
    }

    #[test]
    fn userinfo_at_trick_is_denied() {
        // The classic `allowed@evil` confusion: a human skims "github.com" but
        // the real host is evil.com. `Url::host_str` returns the authority host,
        // not the userinfo, so this is denied.
        assert!(!is_allowed_download_url(
            "https://github.com@evil.com/payload.exe"
        ));
        assert!(!is_allowed_download_url(
            "https://huggingface.co:pass@evil.com/model.bin"
        ));
        // And the inverse must STILL pass: userinfo in front of a truly-allowed
        // host is fine (the host is the allowed one).
        assert!(is_allowed_download_url(
            "https://user@github.com/ggml-org/whisper.cpp/releases/x.zip"
        ));
    }

    #[test]
    fn suffix_confusion_is_denied() {
        // `ends_with(".github.com")` must not be satisfied by a host that merely
        // ENDS with the bare name without the dot boundary.
        assert!(!is_allowed_download_url("https://fakegithub.com/x"));
        assert!(!is_allowed_download_url("https://myhuggingface.co/x"));
        // Garbage / unparseable.
        assert!(!is_allowed_download_url("not a url"));
        assert!(!is_allowed_download_url("https://"));
    }
}
