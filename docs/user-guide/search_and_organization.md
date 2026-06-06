# 🔍 Search & Organization

Phoneme is designed to be your second brain for spoken thought. If you record everything, you need to be able to find everything.

## ⚡ Full-Text Search (FTS5)

At the top of the main UI, you will find a global Search bar.

<!-- SCREENSHOT PLACEHOLDER: Main UI showing the search bar being used -->

Under the hood, Phoneme uses a lightning-fast SQLite FTS5 (Full-Text Search) engine. Every time you transcribe a recording, the text is instantly indexed.

This means:
- **Instant Results**: Searching through 5,000 recordings takes less than 10 milliseconds.
- **Prefix Matching**: If you search for "phono", it will match "phoneme", "phonology", etc.
- **Compound Queries**: You can type multiple words like `"marketing budget"` to find recordings that contain both concepts.

## 🏷️ Organizing with Tags

Tags are the primary way to organize your recordings into projects, categories, or states.

### ➕ Creating and Applying Tags

1. Select one or more recordings from the list (you can `Shift+Click` or `Ctrl+Click` to select multiple).
2. The Action Bar will appear at the top.
3. Click the **Tag** icon.
4. Type a new tag name and press `Enter` to create it, or select an existing tag.

<!-- SCREENSHOT PLACEHOLDER: Action Bar showing the tagging dropdown open -->

### 🎨 The Tag Manager

You can manage your entire tag taxonomy by opening **Settings → Tag Manager**.

<!-- SCREENSHOT PLACEHOLDER: Settings -> Tag Manager screen showing colored tags -->

Here you can:
- **Rename** tags (this will update all associated recordings instantly).
- **Recolor** tags (Phoneme provides a beautiful palette of highly legible colors).
- **Delete** tags (this removes the tag from all recordings, but *does not* delete the recordings themselves).

## 🔎 Filtering Views

You can drill down into your catalog using the Filter pills above the recordings list.

<!-- SCREENSHOT PLACEHOLDER: Main UI showing the "Filter by Tag" and "Date Range" pills -->

- **Tag Filters**: Click "Filter by Tag" to only show recordings that have specific tags attached. You can select multiple tags to narrow your view.
- **Date Filters**: Click the Date pill to restrict your view to "Today", "This Week", or select a custom date range from the calendar.
