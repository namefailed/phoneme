# ⚙️ Phoneme Internals Wiki

This document provides a deep dive into the inner workings of the Phoneme background daemon and core crates.

---

## 🕸️ Async Task Topology & Communication

The daemon is powered by a **Tokio async runtime**, coordinating several long-lived, concurrent background tasks.

```mermaid
flowchart TD
    Pipe[\\.\pipe\phoneme-daemon] -->|Accepts Connections| Server[ipc_server Loop]
    Server -->|Spawns| Handler[ipc_handler Task per Client]
    
    Handler -->|Sends requests| Rec[recorder Actor]
    Handler -->|Pings| Supervisor[whisper_supervisor]
    
    Rec -->|Writes WAV| Queue[Inbox Queue on Disk]
    Queue -->|Triggers| Worker[queue_worker Loop]
    Worker -->|Invokes| Pipeline[pipeline orchestrator]
    
    Pipeline -->|Updates| DB[(SQLite catalog.db)]
    Pipeline -->|Broadcasts events| Bus[event_bus]
    
    Bus -->|Fan-out DaemonEvent| Handler
    Handler -->|NDJSON event stream| Client[Tray App / CLI]
```

### 📡 Communication Channels

Tokio channels are used for thread-safe coordination:

1. **`mpsc` (Multi-Producer Single-Consumer):**
   - Used to send commands (`Stop`, `Cancel`, `Pause`, `Resume`, `Snapshot`) to the [`recorder.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-audio/src/recorder.rs) actor task.
2. **`oneshot`:**
   - Used for quick request-response cycles with the recorder actor (e.g. grabbing an in-progress audio `Snapshot` or getting the final `on_done` signal).
3. **`broadcast`:**
   - The [`event_bus.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/bin/phoneme-daemon/src/event_bus.rs) uses `tokio::sync::broadcast` to broadcast `DaemonEvent` messages. Since multiple clients (tray GUI, CLI commands, external scripts) can subscribe simultaneously, this allows every client to instantly reflect the shared state.
4. **`watch`:**
   - A single process-wide `tokio::sync::watch<bool>` shutdown signal ([`shutdown.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/bin/phoneme-daemon/src/shutdown.rs)) is observed by all tasks. When triggered, loop tasks gracefully drain outstanding blocks and flush the database before exiting.

---

## 🔊 Audio Path details (`phoneme-audio`)

Everything audio converges on the canonical format: **16 kHz, mono, signed 16-bit PCM** (`i16`).

- **Sources:** The [`Source`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-audio/src/source.rs) trait abstracts the capture hardware.
  - `CpalSource` handles microphone input and WASAPI system loopback capture.
  - `SyntheticSource` feeds predetermined waveforms in unit tests, allowing pipeline tests to run on headless runners with no audio hardware.
- **Resampling:** High-quality resampling is driven by `rubato` in [`convert.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-audio/src/convert.rs).
- **Silence Detection:** [`silence.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-audio/src/silence.rs) computes the Root Mean Square (RMS) amplitude of a sliding window. In `oneshot` mode, if the RMS stays below the threshold for the configured duration, the recorder automatically triggers a stop.
- **Pre-Roll Buffer:** [`preroll.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-audio/src/preroll.rs) maintains a ring buffer of the last *N* ms of audio. Idle microphone capture runs silently in the background, recycling samples. On record trigger, the buffer is prepended.
- **Import Decoder:** [`decode.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-audio/src/decode.rs) uses `symphonia` to resample and decode external audio files. A size-limit guard protects against memory exhaustion from malicious or corrupt files.

---

## 🗄️ SQLite Catalog & Search Internals

The database catalog lives in `catalog.db` under local app data ([`catalog.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-core/src/catalog.rs)).

### Connection Settings
The database is opened with:
- **Write-Ahead Logging (WAL) mode** (`journal_mode=WAL`): allows readers to query the database concurrently while a write transaction is in progress.
- **Synchronous Normal** (`synchronous=NORMAL`): avoids disk flushes (fsync) on every single database transaction while retaining complete crash safety in WAL mode.
- **Autocheckpoint Limits** (`wal_autocheckpoint=1000`): keeps the WAL file size around ~4MB. The daemon also fires `checkpoint()` on idle to reclaim disk space.

### Full-Text Search (FTS5) & Triggers
To support fast full-text searching, an FTS5 virtual table (`recordings_fts`) indexes the transcripts. SQLite database triggers keep the FTS5 table in sync with the primary `recordings` table:

```sql
CREATE TRIGGER IF NOT EXISTS recordings_ai AFTER INSERT ON recordings BEGIN
  INSERT INTO recordings_fts(rowid, transcript) VALUES (new.id, new.transcript);
END;
```

To prevent SQL injection or search syntax crashes, user queries are sanitized in [`catalog.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-core/src/catalog.rs):
- Characters that are not alphanumeric are stripped.
- Search words are joined using `* AND ` to build a clean SQLite prefix matching string (e.g. `"data migration"` becomes `"data* AND migration*"`).

---

## 🧠 Semantic Hybrid Search & Fusion

Phoneme uses a **hybrid search** strategy that fuses lexical (keyword) matching with semantic (concept) matching.

```text
               User Query: "database migration"
                            │
              ┌─────────────┴─────────────┐
              ▼                           ▼
      Lexical (FTS5)             Semantic (Embedder)
   Matches exact terms          ONNX Sentence-Transformer
              │                           │
              ▼                           ▼
      lexical_ranking              vector_ranking
              │                           │
              └─────────────┬─────────────┘
                            ▼
                Reciprocal Rank Fusion
                fused_score = 1 / (60 + rank)
                            │
                            ▼
                 [Representative Matches]
```

### 1. Sentence-Aware Chunking
Long recordings are split into overlapping semantic chunks of ~80 words ([`chunk.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-core/src/chunk.rs)). This ensures:
- Small, buried ideas in a 30-minute transcript aren't watered down by the rest of the text.
- We avoid exceeding the token limit of the local ONNX embedding model.

### 2. Reciprocal Rank Fusion (RRF)
RRF merges the ordered ranking lists of the FTS5 retriever and the ONNX vector retriever ([`fusion.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-core/src/fusion.rs)):
- Fused score formula:
  $$score(d) = \sum_{m \in M} \frac{w_m}{k + rank_m(d)}$$
  Where $k = 60.0$ and $w_m$ represents the retriever weight (vector list is weighted at $1.0$, lexical list at $0.85$).

### 3. Relevance Calibration
Raw cosine similarity scores from `all-MiniLM-L6-v2` typically cluster between `0.3` (unrelated) and `0.75` (exact matches). We calibrate these scores into user-friendly percentages using a linear ramp:
- Cosine $\le 0.15$ calibrates to **$0\%$**.
- Cosine $\ge 0.70$ calibrates to **$100\%$**.
- Scores in between scale linearly.
