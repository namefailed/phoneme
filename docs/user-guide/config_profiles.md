# 🎭 Config Profiles

Phoneme lets you save and switch between whole configurations, called
**Profiles**. This is handy when you use Phoneme in more than one setting — say
"Work" and "Home" — and want different behaviour for each.

## ⚙️ What is a Profile?

A profile is a complete snapshot of your `config.toml` saved under
`<config_dir>/profiles/<name>.toml`. Because it captures the *entire* config,
switching profiles can change anything you can configure, including:

- Hotkey bindings
- The active transcription model (e.g. a small, fast model on battery and a
  large one when plugged in)
- LLM Smart Cleanup, summary, auto-tag, and title settings
- Enabled hooks and the hook allowlist
- Which provider does transcription / cleanup / summaries

Switching is a **hot reload** — the daemon picks up the new config instantly,
so you never have to restart Phoneme for a profile change to take effect.

## 🖱️ Managing Profiles in the app

Open **Settings → Managers → Profiles** (or press `g` then `P`). The Profiles
manager lets you:

- **Save** the current live config as a new named profile.
- **Switch** to a saved profile — the active one is marked, and the daemon
  reloads immediately.
- **Update** a profile to overwrite it with your current config.
- **Rename** a profile in place.
- **Delete** a profile you no longer need.

Each profile shows when it was last saved (e.g. "saved 3h ago"), and the
profile you last switched to is flagged as **Active**.

## 🎬 One-click switching from the Record button

You don't have to open Settings to flip profiles. The **Record** split-button has
a small caret next to it — open that dropdown and you'll find a **Capture
profile** section listing every saved profile with a 👤 icon.

Click one and Phoneme swaps the **whole config** to that profile *before your next
capture*, so the recording you're about to start uses that intent's settings.
Think of it as picking the mode for the next take:

- **Standup** — a fast local model, terse Smart Cleanup, auto-tag `#standup`.
- **Interview** — a high-accuracy model with diarization and a summary hook.

Switching here is the **same hot reload** as everywhere else: the click writes the
profile into your live `config.toml` and reloads the daemon on the spot. A small
toast confirms the change (e.g. *"Capture profile: Standup"*), and the rest of the
app re-reads the now-current config immediately — no restart, no extra steps.

> [!NOTE]
> The caret is only available while you're idle. You can't change profile mid-take
> — the menu is disabled once a recording or meeting is running.

**Haven't saved any profiles yet?** The dropdown shows a **Set up profiles…**
entry instead, which jumps straight to **Settings → Managers → Profiles** so you
can create your first one. Once you have profiles, that section also offers a
**Manage profiles…** shortcut to the same place.

## ⌨️ Managing Profiles from the CLI

Everything above is also scriptable, which is what makes profiles useful for
automation.

### Listing profiles

See every saved profile (the active one is indicated):

```bash
phoneme profile list
```

### Saving a profile

Capture the current config as a named snapshot:

```bash
phoneme profile save work_mode
```

### Switching profiles

Switch the active config to a saved profile. The daemon hot-reloads — no
restart needed:

```bash
phoneme profile use work_mode
```

> [!TIP]
> **Automation ideas**
> Because `phoneme profile use` returns immediately and reloads the engine on
> the fly, you can wire it to your OS automation. Bind it to a Windows Task
> Scheduler trigger or an Elgato Stream Deck button to flip Phoneme into a
> "meeting" profile — one that, say, enables an automated meeting-summarizer
> hook — with a single press.
