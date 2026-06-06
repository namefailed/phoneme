# Transcribe-in-Place

Phoneme v1.8 introduces one of the most highly requested features: **Transcribe-in-Place**.

Transcribe-in-Place allows you to use Phoneme as a system-wide dictation engine. Instead of recording a note, letting it transcribe, and then copying the text out of the Phoneme UI, Phoneme will simply simulate the keystrokes to type the transcribed text directly into whatever application or text box you currently have focused.

## How to use it

1. Ensure the feature is enabled in **Settings -> Hotkeys**.
2. By default, the global hotkey is `Ctrl+Alt+I` (though you can change this to anything, like a spare mouse button or F-key).
3. Focus any text field in any application (your browser, Discord, a Word document, your code editor).
4. Press and hold the Transcribe-in-Place hotkey.
5. Speak your thought.
6. Release the hotkey.

Phoneme will silently record your voice, pass it to your active transcription engine (e.g. the blazing fast Native Whisper engine), and instantly type the result into your active window.

## How it works under the hood

When the `Transcribe-in-Place` hotkey is released:
1. The daemon tags the recording with an `in_place: true` flag in the SQLite catalog.
2. The transcript pipeline finishes.
3. Because the `in_place` flag is set, the daemon triggers a simulated OS-level keyboard event sequence. It essentially "pastes" the text by emitting keydown/keyup events for every character in the transcript.
4. The recording is saved to your history just like a normal recording, meaning if you dictated something into a chat window but accidentally closed it, you can always open the Phoneme UI to retrieve your text!

> [!TIP]
> Combine Transcribe-in-Place with the LLM Post-Processing feature to automatically clean up stutters before it gets typed into your window!
