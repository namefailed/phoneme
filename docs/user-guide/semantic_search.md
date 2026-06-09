# Semantic Search

Keyword search (FTS5) finds exact words. **Semantic search** finds recordings by *meaning* — e.g. "that idea about Rust error handling" even if you never said those exact words.

## How it works

When enabled, the daemon loads a small **ONNX embedding model** (all-MiniLM-L6-v2 class) at startup. Each completed transcript is embedded and stored in a vector index alongside the SQLite catalog. Search queries are embedded the same way; results are ranked by cosine similarity.

Everything runs **offline** on your machine. No cloud API.

## Enabling

1. Download or place the ONNX model + tokenizer in a directory (the First Run Wizard can guide this, or set the path manually).
2. Open **Settings → System → Advanced** (or edit `config.toml`):

```toml
[semantic_search]
enabled = true
model_dir = "C:/Users/You/AppData/Local/phoneme/models/all-MiniLM-L6-v2"
```

3. Save and let the daemon reload. New transcripts are indexed automatically; use **Re-index** (if exposed in UI) or restart the daemon to backfill old recordings.

## Using semantic search

In the main search bar, switch the search mode to **Semantic** (or use the semantic filter pill, depending on your version). Type a natural-language query:

- "budget discussion with Sarah"
- "bug where the daemon wouldn't start"
- "recipe for sourdough"

Results include a relevance score. Combine with **tag** and **date** filters to narrow further.

## Comparing FTS5 vs semantic

| | Keyword (FTS5) | Semantic |
|---|----------------|----------|
| **Best for** | Exact phrases, names, IDs | Concepts, paraphrases |
| **Speed** | Sub-10 ms on 5k rows | Slightly heavier (embedding query) |
| **Offline** | Yes | Yes |
| **Index size** | Built into SQLite | Additional vector store |

Use both: keyword for "Q3 roadmap", semantic for "the meeting where we argued about timelines".

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| Semantic toggle grayed out | Set `semantic_search.enabled = true` and a valid `model_dir` |
| No results for old notes | Re-index or re-transcribe to generate embeddings |
| High RAM at startup | Expected — the ONNX model loads once; disable semantic search if RAM is tight |

See also [Search & Organization](search_and_organization.md) and [Configuration Reference](../developer-guide/config_reference.md).
