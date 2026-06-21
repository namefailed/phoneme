# ⌨️ Transcribe-in-Place

Phoneme v1.8 introduces one of the most highly requested features: **Transcribe-in-Place**.

Transcribe-in-Place allows you to use Phoneme as a system-wide dictation engine. Instead of recording a note, letting it transcribe, and then copying the text out of the Phoneme UI, Phoneme will simply simulate the keystrokes to type the transcribed text directly into whatever application or text box you currently have focused.

## 🚀 How to use it

1. Ensure the feature is enabled in **Settings → Hotkeys**.
2. By default, the global hotkey is `Ctrl+Alt+I` (though you can change this to anything, like a spare mouse button or F-key).
3. Focus any text field in any application (your browser, Discord, a Word document, your code editor).
4. Press and hold the Transcribe-in-Place hotkey.
5. Speak your thought.
6. Release the hotkey.

Phoneme will silently record your voice, pass it to your active transcription engine (e.g. the blazing fast Native Whisper engine), and instantly type the result into your active window.

## ⚡ The fast lane

Dictations don't go through the normal processing pipeline. When you release the
hotkey, the recording takes a **dedicated fast lane**:

1. It **skips the queue** — even if a meeting is mid-transcription, your
   dictation transcribes immediately.
2. It uses the **fastest available model**: the Live Preview's dedicated fast
   model when that's enabled, else the main transcription provider. Want
   dictation on its own engine? **Settings → Dictation → Dictation engine →
   Dictation model** switches from Automatic to a dedicated provider — a fast cloud API
   like Groq, or a local whisper server that's already running (the main one
   or the preview's — dictation never starts a third).
3. A **zero-latency polish** cleans the text before it lands: filler words
   ("um", "uh") and whisper's non-speech tags are stripped, stutter-doubled
   words collapsed, capitalization and end punctuation fixed. No AI round-trip
   — it's instant. (Settings → Dictation can switch this to raw
   output, or to a full **AI cleanup** pass if you prefer polish over speed.)
4. The text is **typed** at your cursor — or **pasted** via the clipboard
   (near-instant for long text, with your previous clipboard restored) when
   "Insert text by" is set to Pasting.
5. Only **after** the text has landed does the recording save to your library
   (searchable, with audio) — so if you dictated into a chat you accidentally
   closed, the text is still in Phoneme. Turn "Keep dictations in the library"
   off for fully ephemeral dictation.

## ⌨️ Stream as you speak (experimental)

By default the text lands all at once when you release the key. Turn on
**Settings → Dictation → "Stream as you speak"** (with delivery set to **Typing**)
and the words instead appear **live at your cursor as you speak** — like the
dictation tools built into some operating systems.

It works by typing the live preview's words as they settle, then, the moment you
stop, **quietly patching them up to the accurate final transcript** — usually just
finishing the last few words and the closing punctuation. It only ever *adds* text
while you're speaking (never reaching back to retype), so the cursor doesn't jump
around mid-sentence.

Two things to know: it surfaces the **live preview's** rough first-pass words
(corrected at the end), so it reads best with a **fast preview model**
(Settings → Live Preview); and it's **Typing-only** — it's ignored when delivery
is set to Pasting. It's off by default and clearly marked experimental.

## 🗣️ Voice commands

While dictating, a few spoken commands are turned into formatting instead of
being typed literally:

| Say | Does |
|-----|------|
| "new line" | inserts a line break |
| "new paragraph" | inserts a blank line |
| "scratch that" / "delete that" | removes the sentence you just dictated |

These work in every cleanup mode (Fast, Off, and AI cleanup). To keep normal
speech safe, a command only triggers when it's said on its own — "put it on a
new line of code" mid-sentence is left as written. (With AI cleanup on, the
model is asked to apply them, which handles looser phrasing too.)

### Make them your own

The phrase set is **fully editable** under **Settings → Dictation → Voice
commands**:

- **Add your own wording** — e.g. map `"break here"` to a blank line, or
  `"clear that"` to scratch.
- **Localize** — replace the English phrases with ones in your language.
- **Disable individual commands** — drop a phrase from the list so it's typed
  literally instead.
- **Turn the whole thing off** — the **Interpret spoken commands** toggle types
  every phrase literally without clearing your custom list.

Each command maps to one of three **actions**: a **line break**, a **blank
line**, or **scratch** (drop the sentence you just dictated). Leave the list
empty to use the built-in defaults; **once you add a row, your list replaces the
defaults**, so add the ones you want to keep. A customized map is honored in all
three cleanup modes — including AI cleanup, where your actual phrases are
described to the cleanup model.

In `config.toml` the same map lives under `[in_place.voice_commands]` (phrase →
action), gated by `voice_commands_enabled`:

```toml
[in_place]
voice_commands_enabled = true

[in_place.voice_commands]
"new line"      = "newline"     # keep the defaults you still want…
"new paragraph" = "paragraph"
"scratch that"  = "scratch"
"break here"    = "paragraph"   # …and add your own
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `voice_commands_enabled` | bool | `true` | Master switch. `false` types every phrase literally, regardless of the map. |
| `voice_commands` | map (phrase → `"newline"` \| `"paragraph"` \| `"scratch"`) | `{}` (empty) | Empty = the built-in set. A non-empty map fully replaces the defaults. Phrases are lowercased on load; entries with an unknown action are dropped with a warning (the config still loads). |

> [!NOTE]
> In-place runs its own fast path, so the [live streaming
> preview](streaming_preview_and_preroll.md) is **skipped** during dictation —
> there's no overlay to feed, and skipping it means the preview's per-second
> transcription ticks never compete with your dictation for the single whisper
> permit, so live preview never adds latency to the paste.

Summaries, auto-tags, and hooks do **not** run for dictations on the fast
lane. If you want dictations to behave exactly like normal recordings, enable
**Run the full pipeline** in Settings → Dictation. With it on,
**When to type** picks between two flavors:

- **Type the text immediately** — the fast transcription is typed the moment
  you stop speaking, while the pipeline (cleanup, summary, auto-tags, hooks)
  keeps running in the background for the library copy. Fast-lane feel *and*
  the full automation; the trade-off is that the typed text is the quick
  polish, not the AI cleanup — the cleaned-up version lands in your library.
- **Type only after every step finishes** — the classic behavior: nothing is
  typed until the whole pipeline is done, so the typed text includes the AI
  cleanup. Slow (the dictation waits in the queue behind anything already
  processing), but what lands at the cursor is exactly what lands in the
  library.

## 🎯 Per-app delivery overrides

By default every app gets the same delivery — either typed keystrokes or a
clipboard paste, whichever you picked under **"Insert text by"**. But some apps
are picky: terminals, remote-desktop sessions, secure prompts, and certain
games reject synthetic keystrokes, while a few don't take a programmatic paste
cleanly either. **Per-app overrides** let dictation behave differently
depending on *which app is focused when you stop speaking*.

The focused app is detected from its executable name (lowercased file stem —
`Code.exe` → `code`, `chrome.exe` → `chrome`). When that app has an override,
it wins; every other app falls back to your global setting.

| Override | What it does |
|----------|--------------|
| `type` | Force simulated keystrokes for this app (works where paste doesn't). |
| `paste` | Force a clipboard paste for this app (for apps that drop fast keystrokes). |
| `off` | **Don't auto-deliver at all** — transcribe and save, but never touch the cursor. |

The `off` mode is the escape hatch for apps that flatly reject injected input
(or where you simply never want dictation to land automatically). The words are
*not* lost: the dictation still records, transcribes, and — depending on your
"Keep dictations in the library" setting — saves, so you can copy it out of
Phoneme afterward. It just stays off the cursor.

Overrides apply on **both** delivery paths: the fast lane *and* the full
queued pipeline (when **Run the full pipeline** is on), so an app set to `off`
or `paste` behaves the same no matter which lane a dictation takes.

Configure overrides under `[in_place].app_overrides` in your config file —
a map of executable stem to mode:

```toml
[in_place]
type_mode = "type"            # global default

[in_place.app_overrides]
code        = "paste"         # VS Code: paste long snippets instantly
"keepassxc" = "off"           # password manager: never auto-type
chrome      = "type"          # browser: force keystrokes
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `app_overrides` | map (stem → `"type"` \| `"paste"` \| `"off"`) | `{}` (empty) | Per-app delivery override, keyed by the lowercased executable stem of the app focused when you stop speaking. An unlisted (or undetectable) app uses `type_mode`. |

> [!NOTE]
> Foreground-app detection is **Windows-only**. On other platforms the focused
> app can't be read, so every dictation falls back to the global `type_mode`
> and overrides have no effect.

> [!TIP]
> With `app_overrides` empty (the default), behavior is byte-for-byte unchanged
> — every app uses your global delivery mode exactly as before.

## 🧠 App-aware AI cleanup (opt-in)

When **AI cleanup** is your dictation cleanup mode, Phoneme can optionally tell
the cleanup model *what kind of window you're dictating into*, so it adapts its
polish: leaning code-ish in an editor, prose in a document, terse in a chat. It
does this by prepending the **focused window's title** to the cleanup prompt —
nothing more.

This is **off by default** and privacy-first by design. A window title can be
sensitive (a document name, an email subject, an account in a banking app), so
the feature is strictly opt-in and tightly scoped:

- **It only runs with AI cleanup.** The title is consulted *only* when the
  dictation cleanup mode is the full LLM pass. The Fast and Off cleanup modes
  never read it.
- **The title is read only when you turn it on.** With `app_context` off, the
  title is **never even read** — not captured, not sent, not stored.
- **It goes exactly one place: that single cleanup prompt.** The title is never
  logged, never written to disk, and never saved with the recording. It rides
  along in the one LLM request and is gone.
- **It's sent to your configured cleanup LLM.** If your post-processing
  provider is a cloud API, the title travels there with the prompt. Prefer a
  **local cleanup model** (e.g. Ollama) if titles must never leave your machine.
- **You can exclude specific apps even when it's on** via a denylist — so a
  password manager or banking app never contributes its title, regardless.

```toml
[in_place]
cleanup     = "llm"           # app context only applies to AI cleanup
app_context = true            # opt in

# Apps whose titles are NEVER read, even with app_context on:
app_context_denylist = ["keepassxc", "1password", "mybank"]
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `app_context` | bool | `false` | When on (and the focused app isn't denylisted), prepend the focused window's title to the AI cleanup prompt so the model adapts to what you're working in. Only consulted when `cleanup = "llm"`. |
| `app_context_denylist` | list of strings | `[]` (empty) | Executable stems (lowercased) whose window titles are never read for context, even when `app_context` is on. |

> [!IMPORTANT]
> The window title is treated as potentially sensitive. It is used **solely**
> to flavor one AI cleanup prompt — it is never logged, never persisted, and
> never sent anywhere but to the cleanup model you've configured. Turn
> `app_context` off (the default) and the title is never touched at all.
