# 👥 Meeting Mode (Dual-Track Recording)

Are you tired of taking notes while trying to actively participate in a video call? Phoneme's **Meeting Mode** is designed specifically for this scenario.

Instead of just recording your microphone, Meeting Mode captures *both* your microphone and your computer's system audio (what you hear through your speakers) simultaneously, as two separate but linked tracks.

## ⚙️ How to Enable Meeting Mode

1. Open Phoneme.
2. Click the **Meeting Mode** toggle (the icon next to the main Record button).
3. Phoneme will immediately begin recording two streams:
   - **Mic Track**: Your own voice.
   - **System Track**: The voices of everyone else on the call (Zoom, Teams, Google Meet, etc.).

<!-- SCREENSHOT PLACEHOLDER: Main UI showing the Meeting Mode toggle activated -->

## ✨ Why Dual-Track is Magic

When the meeting ends, Phoneme transcribes both tracks independently. Because they were recorded at the exact same time, Phoneme knows precisely when each track was speaking.

This leads to the **Merged Conversation View**.

```mermaid
%%{init: {'flowchart': {'curve': 'basis', 'useMaxWidth': false}, 'theme': 'dark', 'themeVariables': { 'fontSize': '12px' }}}%%
flowchart LR
    subgraph A [Audio]
        M[🎤 Mic] -->|You| T1[Track 1]
        S[🔊 System] -->|Meeting| T2[Track 2]
    end
    
    subgraph X [Transcribe]
        T1 --> W{Whisper}
        T2 --> W
        W --> R1[Raw 1]
        W --> R2[Raw 2]
    end
    
    subgraph D [Diarize & Merge]
        R2 --> P{Pyannote}
        P -->|Speakers| L[Labels]
        R1 --> M{Merge}
        L --> M
    end
    
    M --> V[Timeline View]
```

Instead of a giant block of text, you get a beautiful, chronological timeline of the conversation, exactly as it happened:

<!-- SCREENSHOT PLACEHOLDER: Merged Conversation View showing "You" and "System" interleaved -->

### 🗣️ Adding Speaker Diarization

If you want to take this to the next level, you can enable **Offline Speaker Diarization** in **Settings → Whisper**.

By default, the System Track is just one long transcript of everyone on the call. But with Diarization enabled, Phoneme uses a powerful AI model (Pyannote) to analyze the System Track and separate the different voices.

Your final transcript will look like this:

- **[You]**: "What do we think about the new design?"
- **[Speaker 1]**: "I love it, but we need to tweak the colors."
- **[Speaker 2]**: "Agreed, let's make it more vibrant."

## 🏆 Best Practices for Meeting Mode

> [!TIP]
> **Use Headphones!** If you use speakers, your microphone will pick up the audio coming from your speakers. This causes an "echo" where the other people on the call are transcribed on *both* the System track and your Mic track. Always wear headphones when using Meeting Mode.

> [!TIP]
> **Combine with Smart Cleanup.** Use the Meeting Summarizer prompt in Smart Cleanup: *"This is a multi-speaker transcript. Provide a concise summary of the decisions made, and list the action items assigned to each speaker."* Phoneme will automatically generate a pristine summary of the entire meeting.
