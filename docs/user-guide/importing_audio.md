# 📥 Importing Audio

Phoneme isn't just for live dictation and meetings. You can also use Phoneme's powerful transcription and LLM post-processing pipeline on audio you've already recorded elsewhere!

## 🎙️ Supported Formats

Phoneme can import and transcribe the following audio formats:
- `.wav`
- `.mp3`
- `.m4a` (AAC / ALAC)
- `.flac`

## 🚀 How to Import

You can feed an existing file directly into Phoneme's pipeline via the CLI:

```bash
phoneme import C:\path\to\my_recording.mp3
```

### What happens when you import?

1. **Decoding**: Phoneme decodes the file and resamples it to its canonical 16 kHz format.
2. **Storage**: It copies the decoded `.wav` file into your Phoneme audio directory, meaning the original file is left completely untouched.
3. **Processing**: The file is queued exactly like a live recording. It will be transcribed by Whisper, cleaned up by your LLM (if enabled), and sent through any of your active Hooks.
4. **Visibility**: The imported recording will immediately show up in the Phoneme GUI alongside your live recordings!

> [!TIP]
> Importing is incredibly useful if you record voice memos on your phone while driving or walking, and want to bulk-process them through Phoneme when you get back to your computer. Just drop the files on your PC and run `phoneme import`!
