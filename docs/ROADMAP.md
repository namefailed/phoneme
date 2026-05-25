# Phoneme Roadmap

This document outlines the short-term, medium-term, and long-term vision for Phoneme.

## 🌟 v1.3.0 (Stable Release)
*Focus: Polish, full feature parity, public launch*
- [x] **Smart Cleanup (AI):** LLM post-processing via local Ollama or OpenAI, with 9 prompt presets.
- [x] **Auto-Updater:** Seamless in-app updates straight from GitHub.
- [x] **11 Themes:** Catppuccin Mocha/Macchiato/Latte, Dracula, Everforest, Gruvbox, Nord, One Dark, Rosé Pine, Solarized Light, Tokyo Night.
- [x] **Vim Mode:** Full Vim emulation in the transcript editor (visual, linewise, mouse selection).
- [x] **Dynamic Layouts:** Resizable, configurable columns in the recordings list.
- [x] **Clipboard Hook:** `to-clipboard.ps1` copies transcript to clipboard instantly.
- [x] **Doctor:** Health checker with one-click daemon restart and Ollama/Whisper probes.
- [x] **Clean Shutdown:** Daemon and whisper-server stop cleanly when the app closes.

## 🚀 v1.4 (Short Term)
*Focus: Accessibility and Offline Capability*
- [ ] **Bundled Ollama Support:** Seamless offline AI post-processing out-of-the-box without requiring manual Ollama setup.
- [ ] **Extended Hook Presets:** More built-in integrations for popular tools (Notion, Obsidian, Discord webhooks).
- [ ] **macOS Beta:** Early macOS port for Apple Silicon.

## 🔮 v2.0 (Medium Term)
*Focus: Platform Expansion and Real-time Processing*
- [ ] **macOS Port:** Full native support for macOS (Intel and Apple Silicon).
- [ ] **Linux Port:** Support for common Linux distributions (X11 / Wayland).
- [ ] **Streaming Transcription:** Watch the transcript generate in real-time as you speak, rather than waiting for the recording to finish.

## 🌌 Long Term Vision
*Focus: Ecosystem and Mobile*
- [ ] **Mobile Thin-Client:** A companion app for iOS and Android that securely syncs voice notes to your desktop daemon.
- [ ] **Plugin Ecosystem:** A standardized plugin system for community-contributed hooks and themes.
