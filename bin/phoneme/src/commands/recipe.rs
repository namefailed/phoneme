//! Local resolution for the `--recipe` flag shared by `record` and
//! `retranscribe`.
//!
//! The daemon's IPC takes a recipe by its stable `id` (see
//! `RecordStart`/`RetranscribeRecording`'s `recipe_id`), but a human picking a
//! recipe on the command line wants to type the name they see in the GUI. The
//! CLI is a local client that reads the same config the daemon does, so it
//! resolves the flag value here — id first, then case-insensitive name — and
//! passes the resolved `id` over the wire. An unmatched value is a hard error
//! rather than a silent fall-back to the default pipeline, and it lists what's
//! available, so a typo'd `--recipe` is caught at the call site instead of
//! quietly running the wrong pipeline.

use phoneme_core::Config;
use std::process::ExitCode;

/// `phoneme recipes` — list the configured Playbook recipes from the same config
/// the daemon reads (no daemon required). With `--json`, prints one JSON array of
/// the full recipe objects (id, name, description, builtin, scope, steps) so a
/// client can build a recipe picker — e.g. filtering `scope == "recording"` for
/// `import --recipe` / `record --recipe`. Otherwise a short human listing.
pub fn list(cfg: &Config, json: bool) -> ExitCode {
    if json {
        return match serde_json::to_string(&cfg.recipes) {
            Ok(s) => {
                println!("{s}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: failed to serialize recipes: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if cfg.recipes.is_empty() {
        println!("(no recipes configured)");
        return ExitCode::SUCCESS;
    }
    for r in &cfg.recipes {
        let scope = match r.scope {
            phoneme_core::config::RecipeScope::Recording => "recording",
            phoneme_core::config::RecipeScope::Meeting => "meeting",
        };
        let builtin = if r.builtin { " · built-in" } else { "" };
        println!("{}  \"{}\"  [{scope}{builtin}]", r.id, r.name);
        if !r.description.trim().is_empty() {
            println!("    {}", r.description);
        }
    }
    ExitCode::SUCCESS
}

/// Resolve a `--recipe` value to a recipe id from `config.recipes`.
///
/// Matches by `id` first (exact), then by `name` (case-insensitive, trimmed).
/// On no match returns an error string naming every available recipe so the
/// user can correct the value.
pub fn resolve(cfg: &Config, value: &str) -> Result<String, String> {
    let needle = value.trim();

    // Id match first — ids are stable and unambiguous.
    if let Some(r) = cfg.recipes.iter().find(|r| r.id == needle) {
        return Ok(r.id.clone());
    }

    // Then name, case-insensitively (trim both sides).
    if let Some(r) = cfg
        .recipes
        .iter()
        .find(|r| r.name.trim().eq_ignore_ascii_case(needle))
    {
        return Ok(r.id.clone());
    }

    let available = if cfg.recipes.is_empty() {
        "(none configured)".to_string()
    } else {
        cfg.recipes
            .iter()
            .map(|r| format!("{} (\"{}\")", r.id, r.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    Err(format!(
        "no recipe matching '{value}'. Available recipes: {available}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config whose recipes are the seeded defaults (`default`,
    /// `meeting_digest`, `standup`, `interview`).
    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn resolves_by_id() {
        let id = resolve(&cfg(), "meeting_digest").expect("id match");
        assert_eq!(id, "meeting_digest");
    }

    #[test]
    fn resolves_by_name_case_insensitively() {
        // "Meeting digest" is the display name of the `meeting_digest` recipe.
        let id = resolve(&cfg(), "  MEETING DIGEST  ").expect("name match");
        assert_eq!(id, "meeting_digest");
    }

    #[test]
    fn id_takes_precedence_over_name() {
        // The `default` recipe's id is "default"; an id hit must win regardless
        // of any name collision.
        let id = resolve(&cfg(), "default").expect("id match");
        assert_eq!(id, "default");
    }

    #[test]
    fn unknown_recipe_errors_and_lists_available() {
        let err = resolve(&cfg(), "does-not-exist").expect_err("must not match");
        assert!(err.contains("does-not-exist"), "names the bad value: {err}");
        assert!(err.contains("Available recipes:"), "lists choices: {err}");
        // At least the default pipeline id must appear in the listing.
        assert!(err.contains("default"), "lists the default recipe: {err}");
    }
}
