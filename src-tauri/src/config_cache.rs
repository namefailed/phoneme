//! Process-wide cached config snapshot for the hot paths.
//!
//! The global-shortcut handler and the window/exit hooks need the live config
//! on every event — a dictation keypress, a window close — but they only read a
//! handful of fields (hotkey combos/modes, `minimize_to_tray`,
//! `quit_stops_daemon`). Re-reading `config.toml` from disk and re-parsing the
//! TOML on each of those events is synchronous I/O on the shortcut callback
//! thread, paid per keypress. This holds one in-memory snapshot instead.
//!
//! The snapshot must track every place config changes so behavior (e.g. "apply
//! a new hotkey combo immediately") is preserved: `apply_config` and
//! `switch_profile` go through the GUI/tray write paths and call `refresh`, and
//! the snapshot is primed once at startup. The cache is the per-user
//! `config.toml` only — same as `config_io` — so it matches `read_or_default`
//! for the tray (which doesn't honor `PHONEME_CONFIG`).

use phoneme_core::Config;
use std::sync::RwLock;

static CACHE: RwLock<Option<Config>> = RwLock::new(None);

/// Replace the cached snapshot with `config`. Call this at every point the live
/// config changes (startup prime, save, profile switch) so the hot-path readers
/// never see a stale combo or toggle.
pub fn set(config: &Config) {
    if let Ok(mut guard) = CACHE.write() {
        *guard = Some(config.clone());
    }
}

/// The cached config snapshot. Falls back to a disk read (then `Config::default`)
/// only when the cache hasn't been primed yet or the lock is poisoned, so the
/// hot paths still behave exactly like the old `read_or_default` in those rare
/// cases instead of silently using defaults.
pub fn get() -> Config {
    if let Ok(guard) = CACHE.read() {
        if let Some(cfg) = guard.as_ref() {
            return cfg.clone();
        }
    }
    Config::read_or_default()
}
