# 📥 Importing Audio

Phoneme isn't just for live dictation and meetings. You can also run audio you've already recorded elsewhere through Phoneme's transcription and LLM post-processing pipeline.

## 🎙️ Supported Formats

Phoneme can import and transcribe the following audio formats:
- `.wav`
- `.mp3`
- `.m4a` (AAC / ALAC)
- `.flac`

## 🚀 How to Import

You have three ways to feed an existing file into Phoneme's pipeline:

### Drag and drop

Drag one or more audio files straight onto the Phoneme window. Each supported
file is imported; anything in an unsupported format is skipped with a quick
note.

### The Import button

Go to **Settings → Storage → Import audio…**. This opens a file picker
filtered to the supported formats; you can select several files at once.

### The CLI

Feed a file in by path — great for scripts and bulk jobs:

```bash
phoneme import C:\path\to\my_recording.mp3
```

### From a URL (YouTube & more)

Give `phoneme import` an `http(s)` URL instead of a file path and it downloads
just the **audio track** with [yt-dlp](https://github.com/yt-dlp/yt-dlp), then
imports it like any local file:

```bash
phoneme import "https://www.youtube.com/watch?v=VIDEO_ID"
```

This is handy for **testing transcription quality**: pull a real-world clip
(interview, podcast, lecture), import it once, then `phoneme retranscribe <id>`
with different models/settings and compare the versions side by side.

By default the audio is extracted to `.m4a` (small, and transparent enough for
speech). For testing where you'd rather avoid any re-encode, pick a lossless
container:

```bash
phoneme import --format flac "https://youtu.be/VIDEO_ID"
# choices: m4a (default), mp3, flac, wav
```

### Pick a Playbook recipe for the import

By default an import runs the **default pipeline**. To run it through a specific
recipe instead — in a single pass, rather than importing then re-transcribing —
add `--recipe` (by id or name, the same picker `record`/`retranscribe` use):

```bash
phoneme import "https://youtu.be/VIDEO_ID" --recipe lecture-clean
phoneme recipes          # list your recipes (add --json for a machine-readable list)
```

The recipe is checked **before** any download, so a typo (or a meeting template,
which can't apply to a single recording) fails fast. Already imported? Change its
recipe with `phoneme retranscribe <id> --recipe <name>`.

### Import once, idempotently (`--ext-ref`)

Scripting bulk imports? Tag each import with your own stable key for the source
and a re-run won't duplicate it:

```bash
phoneme import "https://youtu.be/VIDEO_ID" --ext-ref "yt:VIDEO_ID"
```

If a recording already carries that `--ext-ref` key, the import is a **no-op** that
returns the existing one (`already imported … (matched --ext-ref)`, or
`{"id":…,"reused":true}` with `--json`) instead of importing a second copy. The
key rides `phoneme list --json` as `ext_ref`, so a caller can reconcile what's
already in the library and fire-and-forget the rest.

> [!NOTE]
> URL import requires **yt-dlp** and **ffmpeg** on your PATH. Install yt-dlp with
> `python -m pip install -U yt-dlp` (ffmpeg via your package manager, e.g.
> `winget install Gyan.FFmpeg`). The download goes to a temp folder and is
> deleted after import — Phoneme keeps only its own decoded copy. Only download
> content you have the right to use.

### What happens when you import?

1. **Decoding**: Phoneme decodes the file and resamples it to its canonical 16 kHz format.
2. **Storage**: It copies the decoded `.wav` file into your Phoneme audio directory, meaning the original file is left completely untouched.
3. **Processing**: The file is queued exactly like a live recording. It will be transcribed by Whisper, cleaned up by your LLM (if enabled), and sent through any of your active Hooks.
4. **Visibility**: The imported recording will immediately show up in the Phoneme GUI alongside your live recordings!

> [!TIP]
> Importing helps when you record voice memos on your phone while driving or walking and want to bulk-process them through Phoneme when you get back to your computer. Drop the files on your PC and run `phoneme import`.
