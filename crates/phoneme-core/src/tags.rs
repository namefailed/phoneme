//! The user's tag vocabulary.
//!
//! This module owns the [`Tag`] record — one row of the catalog's `tags` table.
//! The catalog ([`crate::catalog`]) creates, lists, and attaches tags; this is
//! just the shape they take when they cross into the daemon, CLI, and frontend.
//!
//! Tags are case-insensitively unique at the application level ("Code" and
//! "code" are the same tag) and may be attached to many recordings; the
//! many-to-many link lives in the catalog's `recording_tags` table, not here.

use serde::{Deserialize, Serialize};

/// A tag the user can attach to recordings (its catalog `tags` row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    /// Catalog primary key — stable for the tag's lifetime, used to attach,
    /// detach, rename, and merge.
    pub id: i64,
    /// Display name. Unique case-insensitively; the first-created casing wins.
    pub name: String,
    /// Optional CSS colour for the tag pill (e.g. `"#7aa2f7"`), or `None` for
    /// the theme default.
    pub color: Option<String>,
}
