# 🔍 Search & Organization

Phoneme is designed to be your second brain for spoken thought. If you record everything, you need to be able to find everything.

## ⚡ Full-Text Search (FTS5)

At the top of the main UI, you will find a global Search bar.

![Main recordings view](../screenshots/main.png)

Under the hood, Phoneme uses a lightning-fast SQLite FTS5 (Full-Text Search) engine. Every time you transcribe a recording, the text is instantly indexed.

This means:
- **Instant Results**: Searching through 5,000 recordings takes less than 10 milliseconds.
- **Prefix Matching**: If you search for "phono", it will match "phoneme", "phonology", etc.
- **Compound Queries**: You can type multiple words like `"marketing budget"` to find recordings that contain both concepts.

## 🧠 Semantic search (meaning-based)

Keyword search only matches words you actually said. **Semantic search** uses offline ONNX embeddings to find recordings by *concept* — paraphrases, related ideas, vague memories.

Enable in **Settings → System → Advanced** (`semantic_search.enabled = true`). Full guide: [Semantic Search](semantic_search.md).

## 🏷️ Organizing with Tags

Tags are the primary way to organize your recordings into projects, categories, or states.

### ➕ Creating and Applying Tags

1. Select one or more recordings from the list (you can `Shift+Click` or `Ctrl+Click` to select multiple).
2. The Action Bar will appear at the top.
3. Click the **Tag** icon.
4. Type a new tag name and press `Enter` to create it, or select an existing tag.

### 🎨 The Tag Manager

You can manage your entire tag taxonomy from the **Settings → Tags** tab (see [Settings Overview](settings_overview.md)).

Here you can:
- **Rename** tags (this will update all associated recordings instantly).
- **Recolor** tags (Phoneme provides a beautiful palette of highly legible colors).
- **Delete** tags (this removes the tag from all recordings, but *does not* delete the recordings themselves).

### 🪄 Inline tag chips

In a recording's detail view, tags appear as colored chips. Click a chip to edit its name and color inline, or add a new tag right there — no need to open the Tag Manager for quick edits.

## 🔎 Filtering Views

You can drill down into your catalog using the Filter pills above the recordings list.

- **Library filter**: switch between **All**, **Voice Notes** (single recordings), and **Meetings** (multi-track meeting recordings). This mirrors the CLI `phoneme list --kind all|single|meeting`.
- **Tag Filters**: Click "Filter by Tag" to only show recordings that have specific tags attached. You can select multiple tags to narrow your view.
- **Date Filters**: Click the Date pill to restrict your view to "Today", "This Week", or select a custom date range from the calendar.

## 🔁 The Re-run menu

Each recording has a **Re-run** menu for reprocessing without re-recording: **Re-transcribe** (optionally a different model, optionally skip cleanup), **Re-run cleanup** (one-off provider/model/prompt), **Regenerate summary**, and **Re-fire hook**. Overrides apply to that single run and are never saved to config. See [Smart Cleanup](smart_cleanup.md) and [Providers & Models](providers_and_models.md#one-time-overrides-re-run-menu).

## 📦 Bulk actions

Select multiple rows with **Shift+Click** or **Ctrl+A**. The action bar supports batch **delete**, **re-transcribe**, and **export**.

## 📄 Pagination

Large catalogs load in pages with **Load more** at the bottom (infinite scroll). This keeps the UI responsive with thousands of recordings.
