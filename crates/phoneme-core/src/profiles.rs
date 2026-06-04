//! Named config profiles.
//!
//! A profile is a full-config snapshot stored as
//! `<config_dir>/profiles/<name>.toml`. The live config remains
//! `config.toml`; "switching" to a profile copies its contents over
//! `config.toml` and reloads the daemon. The helpers here only manage the
//! `profiles/` directory — copying over the live config + reloading is the
//! caller's job (see `src-tauri/commands.rs` and the CLI).

use crate::config::default_config_path;
use crate::error::{Error, Result};
use crate::Config;
use std::path::PathBuf;

/// Resolve the directory that holds saved profiles: the config dir + `profiles`.
pub fn profiles_dir() -> Option<PathBuf> {
    default_config_path()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .map(|dir| dir.join("profiles"))
}

/// Validate a profile name strictly to prevent path traversal. Allowed:
/// non-empty, ASCII alphanumerics plus `-`, `_`, and spaces. Anything else
/// (path separators, `..`, dots, control chars) is rejected.
pub fn validate_name(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidConfig("profile name cannot be empty".into()));
    }
    if trimmed != name {
        return Err(Error::InvalidConfig(
            "profile name must not have leading or trailing whitespace".into(),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ')
    {
        return Err(Error::InvalidConfig(format!(
            "invalid profile name {name:?}: only letters, digits, spaces, '-' and '_' are allowed"
        )));
    }
    Ok(())
}

fn profile_path(dir: &std::path::Path, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    Ok(dir.join(format!("{name}.toml")))
}

/// List the saved profile names (the `.toml` file stems in `profiles/`),
/// sorted case-insensitively. Returns an empty list if the directory does
/// not exist yet.
pub fn list_profiles() -> Result<Vec<String>> {
    let dir = profiles_dir()
        .ok_or_else(|| Error::Internal("could not resolve profiles directory".into()))?;
    list_profiles_in(&dir)
}

/// Save `config` as a profile named `name` (creating `profiles/` if needed).
pub fn save_profile(name: &str, config: &Config) -> Result<()> {
    let dir = profiles_dir()
        .ok_or_else(|| Error::Internal("could not resolve profiles directory".into()))?;
    save_profile_in(&dir, name, config)
}

/// Load the profile named `name` and parse it as a `Config`.
pub fn load_profile(name: &str) -> Result<Config> {
    let dir = profiles_dir()
        .ok_or_else(|| Error::Internal("could not resolve profiles directory".into()))?;
    load_profile_in(&dir, name)
}

/// Delete the profile named `name`.
pub fn delete_profile(name: &str) -> Result<()> {
    let dir = profiles_dir()
        .ok_or_else(|| Error::Internal("could not resolve profiles directory".into()))?;
    delete_profile_in(&dir, name)
}

// ── Directory-parameterized cores (testable without touching the real
//    config dir) ────────────────────────────────────────────────────────────

fn list_profiles_in(dir: &std::path::Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(names),
        Err(e) => return Err(Error::Io(e)),
    };
    for entry in read {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }
    }
    names.sort_by_key(|n| n.to_lowercase());
    Ok(names)
}

fn save_profile_in(dir: &std::path::Path, name: &str, config: &Config) -> Result<()> {
    let path = profile_path(dir, name)?;
    config.validate()?;
    std::fs::create_dir_all(dir)?;
    let body = toml::to_string_pretty(config)
        .map_err(|e| Error::Internal(format!("failed to serialize profile: {e}")))?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn load_profile_in(dir: &std::path::Path, name: &str) -> Result<Config> {
    let path = profile_path(dir, name)?;
    if !path.exists() {
        return Err(Error::NotFound {
            id: format!("profile {name:?}"),
        });
    }
    Config::load(&path)
}

fn delete_profile_in(dir: &std::path::Path, name: &str) -> Result<()> {
    let path = profile_path(dir, name)?;
    if !path.exists() {
        return Err(Error::NotFound {
            id: format!("profile {name:?}"),
        });
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn validate_accepts_reasonable_names() {
        for ok in ["work", "personal", "my-profile", "test_1", "Work Laptop"] {
            validate_name(ok).unwrap_or_else(|_| panic!("{ok:?} should be valid"));
        }
    }

    #[test]
    fn validate_rejects_empty() {
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
    }

    #[test]
    fn validate_rejects_path_traversal() {
        // Path separators, parent refs, and dots must all be rejected so a
        // crafted name can never escape the profiles directory.
        for bad in [
            "..",
            "../evil",
            "..\\evil",
            "foo/bar",
            "foo\\bar",
            "a.b",
            "C:evil",
            "name\0",
        ] {
            assert!(
                validate_name(bad).is_err(),
                "{bad:?} should be rejected as a traversal risk"
            );
        }
    }

    #[test]
    fn save_load_round_trip() {
        let dir = TempDir::new().unwrap();
        let mut cfg = Config::default();
        cfg.interface.theme = "tokyo-night".into();
        cfg.whisper.provider = crate::config::TranscriptionBackend::Openai;
        cfg.whisper.api_key = "sk-test".into();

        save_profile_in(dir.path(), "work", &cfg).unwrap();
        let loaded = load_profile_in(dir.path(), "work").unwrap();
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn list_returns_sorted_stems_and_skips_non_toml() {
        let dir = TempDir::new().unwrap();
        save_profile_in(dir.path(), "zeta", &Config::default()).unwrap();
        save_profile_in(dir.path(), "Alpha", &Config::default()).unwrap();
        // A stray non-toml file must be ignored.
        std::fs::write(dir.path().join("notes.txt"), "ignore me").unwrap();

        let names = list_profiles_in(dir.path()).unwrap();
        assert_eq!(names, vec!["Alpha".to_string(), "zeta".to_string()]);
    }

    #[test]
    fn list_missing_dir_is_empty() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert!(list_profiles_in(&missing).unwrap().is_empty());
    }

    #[test]
    fn delete_removes_profile() {
        let dir = TempDir::new().unwrap();
        save_profile_in(dir.path(), "temp", &Config::default()).unwrap();
        assert_eq!(list_profiles_in(dir.path()).unwrap(), vec!["temp"]);
        delete_profile_in(dir.path(), "temp").unwrap();
        assert!(list_profiles_in(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn load_missing_profile_is_not_found() {
        let dir = TempDir::new().unwrap();
        let err = load_profile_in(dir.path(), "ghost").unwrap_err();
        assert!(matches!(err, Error::NotFound { .. }));
    }

    #[test]
    fn delete_missing_profile_is_not_found() {
        let dir = TempDir::new().unwrap();
        let err = delete_profile_in(dir.path(), "ghost").unwrap_err();
        assert!(matches!(err, Error::NotFound { .. }));
    }

    #[test]
    fn save_rejects_traversal_name_before_touching_disk() {
        let dir = TempDir::new().unwrap();
        let err = save_profile_in(dir.path(), "../escape", &Config::default()).unwrap_err();
        assert!(matches!(err, Error::InvalidConfig(_)));
    }
}
