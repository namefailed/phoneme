# Semantic Search

Keyword search (FTS5) finds exact words. **Semantic search** finds recordings by *meaning* — e.g. "that idea about Rust error handling" even if you never said those exact words.

## How it works

When enabled, the daemon loads a small **ONNX embedding model** (all-MiniLM-L6-v2 by default) at startup. Phoneme then indexes your transcripts for meaning, not just text:

- **Sentence-aware chunking.** Each transcript is split into overlapping, sentence-aware chunks of ~80 words. Every chunk is embedded into its own vector (stored in the `embedding_chunks` table). A recording is scored by its **best-matching chunk** rather than by one averaged whole-transcript vector — so a single idea buried in a long note still ranks, and nothing past the model's token limit is silently dropped.
- **Hybrid retrieval (semantic + keyword).** At query time the semantic ranking is **fused with the FTS5 keyword ranking** using Reciprocal Rank Fusion (RRF). Vector search recalls paraphrases; keyword search nails exact terms it has never seen (proper nouns, code identifiers, acronyms). Fusing both gives the union of their strengths — you get the recording whether you remember the *gist* or the one distinctive word.
- **Calibrated relevance.** Raw cosine similarity isn't intuitively a percentage, so Phoneme calibrates it into a 0–100% relevance score shown as a chip in the results list.

Everything runs **offline** on your machine. No cloud API.

## Enabling

1. Download or place the ONNX model + tokenizer in a directory (the First Run Wizard can guide this, or set the path manually).
2. Open **Settings** and search for **"Semantic"** to reveal the **Semantic Search** section (or edit `config.toml` directly):

```toml
[semantic_search]
enabled = true
model_dir = "C:/Users/You/AppData/Local/phoneme/models/all-MiniLM-L6-v2"
```

3. Save and let the daemon reload. New transcripts are indexed automatically. To backfill or re-index existing recordings, use **Re-embed all recordings** (see below).

## Choosing an embedding model

The default is **all-MiniLM-L6-v2** (384-dim), but you can point Phoneme at any compatible ONNX sentence-embedding model — including instruction-tuned ones like E5 or BGE. The **Semantic Search** settings section (and the `[semantic_search]` table) exposes the knobs each model needs:

| Setting | `config.toml` key | What it does |
|---|---|---|
| Max tokens | `max_tokens` | Truncation length (all-MiniLM was trained at 256). |
| Pooling | `pooling` | `mean` (MiniLM/MPNet/E5/BGE) or `cls`. |
| Token type ids | `token_type_ids` | On for BERT-family models (MiniLM, MPNet); off for E5 exports that reject the input. |
| Query prefix | `query_prefix` | Prepended to a search **query** before embedding (e.g. `query: ` for E5). |
| Passage prefix | `passage_prefix` | Prepended to a stored **transcript** before embedding (e.g. `passage: ` for E5). |

Every field defaults to the all-MiniLM behaviour, so an existing config keeps working unchanged.

> [!IMPORTANT]
> Different models produce vectors of a **different dimension**, which makes your old embeddings unsearchable. After changing the model (or any of the knobs above), click **Save**, then run **Re-embed all recordings**.

## Re-embedding the library

The **Re-embed all recordings** button in the Semantic Search settings section (IPC `ReembedAll`) clears every stored embedding and re-indexes the whole library with the currently-configured model, in the background. Use it to:

- backfill recordings made before you enabled semantic search, or
- migrate the index after switching embedding models.

It returns immediately; indexing continues in the background.

## Using semantic search

In the main search bar, switch the search mode to **Semantic**. Type a natural-language query:

- "budget discussion with Sarah"
- "bug where the daemon wouldn't start"
- "recipe for sourdough"

Results include a calibrated relevance chip (e.g. "87% match"). Combine with **tag** and **date** filters to narrow further.

## Comparing FTS5 vs semantic

| | Keyword (FTS5) | Semantic (hybrid) |
|---|----------------|----------------|
| **Best for** | Exact phrases, names, IDs | Concepts, paraphrases |
| **Speed** | Sub-10 ms on 5k rows | Slightly heavier (embedding query) |
| **Offline** | Yes | Yes |
| **Index size** | Built into SQLite | Per-chunk vector store (`embedding_chunks`) |

Semantic mode already *fuses in* the keyword ranking, so it's the right default for "the meeting where we argued about timelines". Use plain keyword mode when you want only exact matches for "Q3 roadmap".

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| Semantic toggle grayed out | Set `semantic_search.enabled = true` and a valid `model_dir` |
| No results for old notes | Run **Re-embed all recordings** to backfill chunk embeddings |
| Results vanished after changing the model | A new model changes the vector dimension — **Re-embed all recordings** |
| High RAM at startup | Expected — the ONNX model loads once; disable semantic search if RAM is tight |

See also [Search & Organization](search_and_organization.md) and [Configuration Reference](../developer-guide/config_reference.md).
