# 🎭 Config Profiles

Phoneme allows you to save and switch between multiple configurations, called **Profiles**. This is especially useful if you use Phoneme in different environments (e.g., "Work" vs "Home") and want different behaviors for each.

## ⚙️ What is a Profile?

A Profile is a snapshot of your entire configuration, including:
- Hotkey bindings
- The active transcription model (e.g., pointing to a faster, smaller model on battery, and a huge model when plugged in)
- LLM Smart Cleanup settings
- Enabled hooks and plugins

## 📝 Managing Profiles

Currently, profiles are managed via the CLI.

### Listing Profiles

To see all your saved profiles and which one is currently active:
```bash
phoneme profile list
```

### Applying a Profile

When you apply a profile, the daemon instantly hot-reloads the new configuration. You do not need to restart Phoneme for the changes to take effect!
```bash
phoneme profile apply work_mode
```

> [!TIP]
> **Automation Ideas**
> You can hook `phoneme profile apply` up to your OS automation tools. For example, use Task Scheduler on Windows or an Elgato Stream Deck to switch Phoneme into "Meeting Mode" (which might apply a profile that runs an automated meeting-summarizer hook) with a single button press.
