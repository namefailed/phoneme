# ⌨️ Transcribe-in-Place

Phoneme v1.8 introduces one of the most highly requested features: **Transcribe-in-Place**.

Transcribe-in-Place allows you to use Phoneme as a system-wide dictation engine. Instead of recording a note, letting it transcribe, and then copying the text out of the Phoneme UI, Phoneme will simply simulate the keystrokes to type the transcribed text directly into whatever application or text box you currently have focused.

## 🚀 How to use it

1. Ensure the feature is enabled in **Settings → Capture → Hotkeys**.
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
   dictation on its own engine? **Settings → Capture → Dictation → Dictation
   model** switches from Automatic to a dedicated provider — a fast cloud API
   like Groq, or a local whisper server that's already running (the main one
   or the preview's — dictation never starts a third).
3. A **zero-latency polish** cleans the text before it lands: filler words
   ("um", "uh") and whisper's non-speech tags are stripped, stutter-doubled
   words collapsed, capitalization and end punctuation fixed. No AI round-trip
   — it's instant. (Settings → Capture → Dictation can switch this to raw
   output, or to a full **AI cleanup** pass if you prefer polish over speed.)
4. The text is **typed** at your cursor — or **pasted** via the clipboard
   (near-instant for long text, with your previous clipboard restored) when
   "Insert text by" is set to Pasting.
5. Only **after** the text has landed does the recording save to your library
   (searchable, with audio) — so if you dictated into a chat you accidentally
   closed, the text is still in Phoneme. Turn "Keep dictations in the library"
   off for fully ephemeral dictation.

Summaries, auto-tags, and hooks do **not** run for dictations on the fast
lane. If you want dictations to behave exactly like normal recordings (every
configured step, typed only at the very end), enable **Run the full pipeline**
in Settings → Capture → Dictation.
