//! Single source of truth for WHERE secret values live in a serialized
//! [`crate::Config`].
//!
//! Every redactor drives off [`secret_locations`] so the masks can't drift:
//! the CLI `phoneme config` dump (which walks a `toml_edit` document) and the
//! Tauri config masker (which walks a `serde_json::Value` before it crosses into
//! the WebView). A new secret-bearing field is masked in both places by adding a
//! single entry here — previously the two lists were hand-maintained and one had
//! already drifted (`webhook.custom_headers` was masked in the GUI but printed in
//! cleartext by `phoneme config`).

/// One place a secret can live in a serialized `Config`.
#[derive(Debug, Clone, Copy)]
pub enum SecretLocation {
    /// A scalar string secret at this dotted path, e.g. `whisper.api_key`.
    Field(&'static [&'static str]),
    /// A table at this path whose every string value is a secret, e.g.
    /// `webhook.custom_headers` (header values routinely carry bearer tokens).
    MapValues(&'static [&'static str]),
    /// For each element of the array at `path`, the scalar secret at `field`,
    /// e.g. `playbook[].llm.api_key`.
    ArrayField {
        /// Dotted path to the array of tables.
        path: &'static [&'static str],
        /// Dotted path to the secret within each element.
        field: &'static [&'static str],
    },
}

/// Every secret location in a serialized [`crate::Config`] — the single list
/// both redactors consume. Add a row here when a new secret-bearing field lands.
pub fn secret_locations() -> &'static [SecretLocation] {
    use SecretLocation::*;
    &[
        Field(&["whisper", "api_key"]),
        Field(&["preview_whisper", "api_key"]),
        Field(&["llm_post_process", "api_key"]),
        Field(&["summary", "api_key"]),
        Field(&["auto_tag", "api_key"]),
        Field(&["title", "api_key"]),
        Field(&["in_place", "stt", "api_key"]),
        Field(&["webhook", "hmac_secret"]),
        MapValues(&["webhook", "custom_headers"]),
        ArrayField {
            path: &["playbook"],
            field: &["llm", "api_key"],
        },
    ]
}

/// Mask every secret value in a config serialized as JSON, in place, replacing
/// each non-empty string secret with `replacement`. Empty values are left alone
/// (an unset key is not a secret). Driven by [`secret_locations`]; used by the
/// Tauri layer before the config reaches the WebView.
pub fn mask_json(v: &mut serde_json::Value, replacement: &str) {
    for loc in secret_locations() {
        match loc {
            SecretLocation::Field(path) => {
                if let Some(item) = nav_json_mut(v, path) {
                    mask_json_value(item, replacement);
                }
            }
            SecretLocation::MapValues(path) => {
                if let Some(obj) = nav_json_mut(v, path).and_then(|t| t.as_object_mut()) {
                    for (_, val) in obj.iter_mut() {
                        mask_json_value(val, replacement);
                    }
                }
            }
            SecretLocation::ArrayField { path, field } => {
                if let Some(arr) = nav_json_mut(v, path).and_then(|a| a.as_array_mut()) {
                    for entry in arr.iter_mut() {
                        if let Some(item) = nav_json_mut(entry, field) {
                            mask_json_value(item, replacement);
                        }
                    }
                }
            }
        }
    }
}

fn mask_json_value(item: &mut serde_json::Value, replacement: &str) {
    if item.as_str().is_some_and(|s| !s.is_empty()) {
        *item = serde_json::Value::String(replacement.to_string());
    }
}

fn nav_json_mut<'a>(
    v: &'a mut serde_json::Value,
    path: &[&str],
) -> Option<&'a mut serde_json::Value> {
    let mut cur = v;
    for key in path {
        cur = cur.get_mut(key)?;
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A JSON config with a known sentinel at EVERY secret location must come out
    /// with none of them surviving — the guard that keeps a redactor from missing
    /// a field as new ones are added.
    #[test]
    fn mask_json_redacts_every_secret_location() {
        let sentinel = "SECRET_SENTINEL_xyz";
        let mut v = json!({
            "whisper": { "api_key": sentinel },
            "preview_whisper": { "api_key": sentinel },
            "llm_post_process": { "api_key": sentinel },
            "summary": { "api_key": sentinel },
            "auto_tag": { "api_key": sentinel },
            "title": { "api_key": sentinel },
            "in_place": { "stt": { "api_key": sentinel } },
            "webhook": {
                "hmac_secret": sentinel,
                "custom_headers": { "Authorization": format!("Bearer {sentinel}"), "X-Api-Key": sentinel },
            },
            "playbook": [
                { "llm": { "api_key": sentinel } },
                { "llm": { "api_key": sentinel } },
            ],
        });
        mask_json(&mut v, "<redacted>");
        let dumped = serde_json::to_string(&v).unwrap();
        assert!(
            !dumped.contains(sentinel),
            "a secret survived masking: {dumped}"
        );
    }

    /// Empty values are left as-is (an unset key is not a secret).
    #[test]
    fn mask_json_leaves_empty_values() {
        let mut v = json!({ "whisper": { "api_key": "" } });
        mask_json(&mut v, "<redacted>");
        assert_eq!(v["whisper"]["api_key"], json!(""));
    }
}
