//! Output formatting — pretty tables (comfy-table) + JSON-lines.

use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use phoneme_core::Recording;
use serde_json::Value;

/// Print one recording in pretty form.
#[allow(dead_code)]
pub fn print_recording_pretty(r: &Recording) {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table.add_row(vec!["id", r.id.as_str()]);
    table.add_row(vec!["started_at", &r.started_at.to_rfc3339()]);
    table.add_row(vec!["duration", &format_duration(r.duration_ms)]);
    table.add_row(vec!["status", r.status.as_str()]);
    table.add_row(vec!["audio_path", &r.audio_path]);
    if let Some(t) = &r.transcript {
        table.add_row(vec!["transcript", t]);
    }
    if let Some(s) = &r.summary {
        table.add_row(vec!["summary", s]);
    }
    if let Some(n) = &r.notes {
        if !n.is_empty() {
            table.add_row(vec!["notes", n]);
        }
    }
    if let Some(ek) = &r.error_kind {
        table.add_row(vec!["error_kind", ek]);
    }
    if let Some(em) = &r.error_message {
        table.add_row(vec!["error_message", em]);
    }
    println!("{table}");
}

/// Print a list of recordings as a table.
#[allow(dead_code)]
pub fn print_list_pretty(rows: &[Recording]) {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec!["time", "dur", "status", "transcript"]);
    for r in rows {
        let preview = match &r.transcript {
            Some(t) if t.len() > 60 => format!("{}…", &t[..60]),
            Some(t) => t.clone(),
            None => String::new(),
        };
        table.add_row(vec![
            r.started_at.format("%Y-%m-%d %H:%M:%S").to_string(),
            format_duration(r.duration_ms),
            r.status.as_str().to_string(),
            preview,
        ]);
    }
    println!("{table}");
}

/// Print as JSON-lines (one row per line).
#[allow(dead_code)]
pub fn print_json_lines<T: serde::Serialize>(items: &[T]) {
    for item in items {
        if let Ok(line) = serde_json::to_string(item) {
            println!("{line}");
        }
    }
}

/// Print a JSON value as a single line.
#[allow(dead_code)]
pub fn print_json(value: &Value) {
    if let Ok(s) = serde_json::to_string(value) {
        println!("{s}");
    }
}

pub fn format_duration(ms: i64) -> String {
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    let frac = (ms % 1000) / 100;
    if mins > 0 {
        format!("{mins}m{secs:02}.{frac}s")
    } else {
        format!("{secs}.{frac}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_seconds_only() {
        assert_eq!(format_duration(8470), "8.4s");
    }

    #[test]
    fn format_duration_minutes() {
        assert_eq!(format_duration(125_300), "2m05.3s");
    }
}
