# 🚀 Getting Started with Phoneme

Welcome to Phoneme! Phoneme is built on a fundamental philosophy: **Build it your own way.** We provide the tools—lightning-fast transcription, offline privacy, extensive post-processing, and limitless extensibility—but *you* decide how they fit together. 

Whether you want a 100% offline, privacy-first setup that runs entirely on your local hardware, or you want to connect to the fastest cloud APIs for instant results, Phoneme supports it.

## 🧙‍♂️ The First Run Wizard

When you launch Phoneme for the first time, you will be greeted by the First Run Wizard. This wizard is designed to get you up and running with the best possible configuration for your specific hardware.

<!-- SCREENSHOT PLACEHOLDER: First Run Wizard Intro Screen -->

### Step 1: Choosing Your Mode

Phoneme will ask you how you intend to use the application. You can choose from:
1. **🏠 Local & Private (Recommended)**: Phoneme will download an optimized `whisper.cpp` model to your machine. Your audio never leaves your computer.
2. **☁️ Cloud Speed**: If you have an OpenAI, Groq, or Deepgram API key, you can plug it in here for near-instantaneous cloud transcription.
3. **⚙️ Advanced Setup**: Skip the wizard and configure everything manually in Settings.

### Step 2: Hardware Auto-Detection

If you chose the Local & Private route, Phoneme will analyze your system's RAM and VRAM (Video RAM). 

<!-- SCREENSHOT PLACEHOLDER: Wizard Hardware Detection Screen -->

Because AI models require memory to run efficiently, Phoneme uses this data to automatically recommend the optimal Whisper model size for your machine:
- **Tiny / Base**: Best for older laptops or systems with less than 8GB of RAM. Extremely fast, but slightly less accurate.
- **Small / Medium**: The sweet spot for most modern computers. Excellent accuracy and good speed.
- **Large-v3**: Requires significant RAM/VRAM, but offers state-of-the-art accuracy across multiple languages.

You can accept the recommendation, or manually choose a different model. Phoneme will then download the model directly to your device.

## 🎤 Making Your First Recording

Once the wizard completes, you'll land on the main Recordings View.

<!-- SCREENSHOT PLACEHOLDER: Main UI Empty State with big Record button -->

1. Click the large **Record** button at the top of the window, or press the global hotkey (default is `Ctrl+Alt+R`, though you can change this in **Settings → Hotkeys**).
2. Speak clearly into your microphone.
3. You will see your words appear on the screen in real-time as Phoneme's native streaming engine processes the audio.
4. Click **Stop** (or press the hotkey again).

Phoneme will finalize the recording, apply any Smart Cleanup you have configured, and save it to your local SQLite catalog. 

## The Detail Pane

Clicking on any recording in your list will open the **Detail Pane**.

<!-- SCREENSHOT PLACEHOLDER: Detail pane showing waveform, transcript, and notes -->

Here you can:
- **Listen Back**: Click the play button on the interactive waveform to hear your original audio.
- **Edit**: Spot a mistake? Click into the transcript to fix it. Phoneme always saves your original raw transcript in the background, so you never lose data.
- **Take Notes**: Use the free-form Notes text area to jot down thoughts related to the recording. This field is yours and is never overwritten by AI or re-transcription.

## ⏭️ Next Steps

Now that you've mastered the basics, explore Phoneme's power-user features:
- **[👥 Meeting Mode (Dual-Track)](meeting_mode.md)**: Record both sides of a conversation and let AI separate the speakers.
- **[✨ Smart Cleanup (LLM Post-Processing)](smart_cleanup.md)**: Automatically remove stutters, format notes, or translate your voice.
- **[⌨️ Transcribe-in-Place](transcribe_in_place.md)**: Dictate directly into any application on your computer.
- **[🔍 Search & Organization](search_and_organization.md)**: Master tags and full-text search.
