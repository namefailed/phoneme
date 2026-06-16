//! (split from the former commands.rs god-file — see mod.rs)

use super::*;

#[tauri::command]
pub fn reveal_file(path: String) -> Result<(), CommandError> {
    // Security: the renderer can pass any string here and we hand it to
    // `explorer /select`. Restrict the target to the configured audio directory
    // (the only thing the UI ever reveals — a recording's WAV or the folder
    // itself) so a compromised WebView can't pop Explorer onto arbitrary paths.
    let cfg = config_io::read().map_err(|e| format!("config error: {e}"))?;
    // Expand %VAR%/~ in the configured audio dir before comparing. The path the
    // UI reveals is an absolute, already-expanded path, so a raw config string
    // like "%USERPROFILE%\\Documents\\phoneme\\audio" would never match and the
    // reveal would fail "path not permitted".
    let audio_dir_raw = cfg
        .expanded()
        .map(|c| c.recording.audio_dir)
        .unwrap_or_else(|_| cfg.recording.audio_dir.clone());
    let audio_dir = std::path::PathBuf::from(&audio_dir_raw);
    let requested = std::path::PathBuf::from(&path);
    if requested != audio_dir && !path_within(&requested, &audio_dir) {
        return Err("path not permitted".into());
    }

    #[cfg(target_os = "windows")]
    {
        let path = path.replace("/", "\\");
        std::process::Command::new("explorer")
            .args(["/select,", &path])
            .spawn()
            .map_err(|e| format!("failed to open explorer: {}", e))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Fallback for macOS/Linux if ever needed
        let _ = path;
    }
    Ok(())
}

#[tauri::command]
pub fn read_file_string(path: String) -> Result<String, CommandError> {
    // Security: this command exists only to load the user's configured external
    // vimrc. Restrict it to exactly that file (canonicalized) so a compromised
    // renderer cannot read arbitrary files like ~/.ssh/id_rsa.
    let cfg = config_io::read().map_err(|e| format!("config error: {e}"))?;
    if cfg.editor.vimrc_path.is_empty() {
        return Err("no external vimrc is configured".into());
    }
    let allowed =
        std::fs::canonicalize(&cfg.editor.vimrc_path).map_err(|e| format!("config error: {e}"))?;
    let requested = std::fs::canonicalize(&path)
        .map_err(|e| CommandError::from(format!("failed to read {}: {}", path, e)))?;
    if requested != allowed {
        return Err("path not permitted".into());
    }
    std::fs::read_to_string(&requested)
        .map_err(|e| CommandError::from(format!("failed to read {}: {}", path, e)))
}

/// Tail the last `max_lines` of a daemon log file, for the in-app log viewer
/// (Settings → Destination & Integrations → "View hook log"). Restricted to a
/// fixed allowlist of log basenames — no caller-supplied path, no traversal.
///
/// The daemon writes its own log via a daily rolling appender
/// (`daemon.log.YYYY-MM-DD`), so when the exact name is missing we fall back to
/// the newest `<name>*` file in the logs dir (the date suffix sorts as age).
/// Returns "" when no matching log exists yet, so the viewer shows an honest
/// empty state instead of an error.
#[tauri::command]
pub fn tail_log(name: String, max_lines: usize) -> Result<String, CommandError> {
    const ALLOWED: &[&str] = &["hook.log", "daemon.log", "ollama.log"];
    if !ALLOWED.contains(&name.as_str()) {
        return Err("log not permitted".into());
    }
    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| "could not resolve project directories".to_string())?;
    let logs = dirs.data_local_dir().join("logs");
    let mut path = logs.join(&name);
    if !path.exists() {
        // Newest rolled variant, or nothing yet. Accept ONLY `<name>` or
        // `<name>.<digits-and-dashes>` (the daily appender's
        // `daemon.log.YYYY-MM-DD`) so an odd-suffixed or symlinked file dropped
        // in the logs dir can't be selected and read.
        let newest = std::fs::read_dir(&logs).ok().and_then(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    let fname = e.file_name();
                    let Some(f) = fname.to_str() else { return false };
                    if f == name {
                        return true;
                    }
                    match f.strip_prefix(&name).and_then(|r| r.strip_prefix('.')) {
                        Some(suffix) => {
                            !suffix.is_empty()
                                && suffix.bytes().all(|b| b.is_ascii_digit() || b == b'-')
                        }
                        None => false,
                    }
                })
                .max_by_key(|e| e.file_name())
        });
        match newest {
            Some(e) => path = e.path(),
            None => return Ok(String::new()),
        }
    }
    // Defense-in-depth: the resolved file must canonicalize to something inside
    // the logs dir, so a symlink in the logs dir can't redirect the read
    // elsewhere on disk. Treat any mismatch as "no log" rather than leaking.
    match (std::fs::canonicalize(&path), std::fs::canonicalize(&logs)) {
        (Ok(p), Ok(l)) if p.starts_with(&l) => {}
        _ => return Ok(String::new()),
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| CommandError::from(format!("failed to read {}: {}", path.display(), e)))?;
    let max = max_lines.clamp(1, 5000);
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(max);
    Ok(lines[start..].join("\n"))
}

#[tauri::command]
pub fn open_file(path: String) -> Result<(), CommandError> {
    let requested = std::path::Path::new(&path);
    if !requested.exists() {
        return Err(format!("File does not exist: {}", path).into());
    }
    // Security: same contract as reveal_file — the renderer can pass
    // any string and we hand it to the OS. Restrict to the places the UI
    // actually opens: the audio library, phoneme's data dir (logs, models,
    // hooks), and its config dir.
    let cfg = config_io::read().map_err(|e| format!("config error: {e}"))?;
    let audio_dir_raw = cfg
        .expanded()
        .map(|c| c.recording.audio_dir)
        .unwrap_or_else(|_| cfg.recording.audio_dir.clone());
    let mut roots = vec![std::path::PathBuf::from(audio_dir_raw)];
    if let Some(dirs) = directories::ProjectDirs::from("", "", "phoneme") {
        roots.push(dirs.data_local_dir().to_path_buf());
        roots.push(dirs.config_dir().to_path_buf());
    }
    if !roots.iter().any(|r| path_within(requested, r)) {
        return Err("path not permitted".into());
    }
    #[cfg(target_os = "windows")]
    {
        // Use explorer.exe directly instead of `cmd /c start`: the latter runs
        // through the shell, so a filename containing `&` or `"` could be parsed
        // as commands. explorer takes the path literally — no shell layer.
        std::process::Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("failed to open file: {}", e))?;
    }
    Ok(())
}

/// Open the user's hooks directory in the file manager, creating it if missing.
///
/// The Doctor "Fix" button previously passed literal `%LOCALAPPDATA%`/`%APPDATA%`
/// strings to `open_file`, which does no env-var expansion — so the path never
/// existed and nothing opened. Resolve the real directory here instead: it lives
/// under the per-user config dir (`config_dir()/hooks`), matching where the
/// daemon's first-run copy writes the reference hooks.
#[tauri::command]
pub fn open_hooks_folder() -> Result<(), CommandError> {
    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| CommandError::from("could not resolve project directories"))?;
    let hooks_dir = dirs.config_dir().join("hooks");
    std::fs::create_dir_all(&hooks_dir)
        .map_err(|e| CommandError::from(format!("failed to create hooks dir: {e}")))?;
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&hooks_dir)
            .spawn()
            .map_err(|e| CommandError::from(format!("failed to open hooks folder: {e}")))?;
    }
    Ok(())
}
