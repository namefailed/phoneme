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

## "More like this"

Open a recording and you can jump straight from it to everything else you've said on the topic — no query to type:

- **Where:** the **✨ Similar** button in the recording detail's action row (next to Copy/Export), and in the merged meeting view's header. The CLI equivalent is `phoneme search --like <RECORDING_ID>`.
- **What happens:** the recordings list fills with the semantically closest recordings, ranked with the same relevance chips as a semantic query. The search box becomes a **`~similar: <title>`** pill — click its **✕** to return to the normal list.
- **How it's ranked:** the recording's *already-stored* chunk vectors are averaged into one query vector, and the library is scored by each recording's best-matching chunk — the same retrieval path a typed semantic query uses. Nothing is re-embedded, so the lookup is instant and works even while the embedding model isn't loaded.
- **What's excluded:** the source recording itself — and, for a meeting track, the *other* track of the same meeting (its transcript is near-identical, so it would always uselessly rank first).
- **Not indexed yet?** A recording that has no embeddings (recorded before semantic search was enabled, or still in the pipeline) reports *"isn't indexed for semantic search yet — re-embed the library or wait for the pipeline to index it"*. Run **Re-embed all recordings** (above) to backfill.

Because the source's stored vectors are the query, "More like this" only requires that the **source** recording is indexed; candidates are whatever else has vectors.

## Ask your archive

Search returns *recordings*. **Ask** returns an *answer* — a short, written reply
to a question, drawn from your own transcripts and **cited** back to the
recordings it came from. It's local RAG (retrieval-augmented generation): Phoneme
retrieves the most relevant transcript chunks with the **same hybrid retriever as
search** (vector + FTS5, fused with RRF), feeds them to your configured cleanup
LLM provider as grounding, and streams back an answer whose inline `[1]` `[2]`
markers link to the source recordings.

Ask needs two things turned on:

- **Semantic search enabled** (the embedder loads the model and indexes chunks —
  the retrieval half), and
- **an LLM post-processing provider configured** (the same provider Smart Cleanup
  and summaries use — the generation half). With **Local Ollama** the whole thing
  runs offline; nothing leaves your machine.

### From the app

Click the **💬** button in the header to open the **Ask** panel. Type a question
and submit. The panel lists the **citation sources** first, then streams the
answer; each `[n]` chip in the answer is clickable and opens that source
recording in the detail pane. If nothing matched, it tells you so rather than
inventing an answer.

### From the CLI

```bash
# Answer a question from your transcripts, with citations
phoneme ask "what did we decide about the Q3 timeline?"

# Pull more grounding chunks (default 8; clamped server-side)
phoneme ask "open questions on the rewrite" --top-k 12

# Scope the evidence the same way search does — by tag, status, or kind
phoneme ask "action items from the standups" --tag work --kind meeting
phoneme ask "what broke last week" --status done
```

`--tag` takes a tag id or name, `--status` a recording status (e.g. `done`,
`transcribe_failed`), and `--kind` is `single` or `meeting` — all mirror the
[`phoneme search`](search_and_organization.md#-full-text-search-fts5) filters, so
Ask is grounded on exactly the slice you'd have searched.

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

## See also

- [Ask your archive](#ask-your-archive) — get a written, cited answer instead of a list of recordings.
- [Search & Organization](search_and_organization.md) — keyword search, tags, filters, saved searches.
- [Configuration Reference](../developer-guide/config_reference.md) — every `[semantic_search]` key.
