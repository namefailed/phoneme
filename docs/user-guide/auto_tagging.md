# ✨ Auto-Tagging

Phoneme can run each new transcript through your AI provider and **propose
tags** for it. Proposals are exactly that — they appear as dashed ✨ chips in
the recording's tag row and **nothing is applied until you approve it**.

## How it works

1. After a recording is transcribed (and cleaned up / summarized, when those
   are on), the auto-tag step sends the transcript to your LLM along with the
   **complete list of tags you already use**.
2. The model is instructed to **prefer your existing tags** and only invent a
   new one when nothing you have fits. Suggestions are capped (default 5),
   deduplicated, and anything the recording already has is filtered out.
3. The suggestions appear in the recording's tag row as dashed chips:
   - **✓** applies one — the tag is created if it doesn't exist yet, attached,
     and the chip becomes a real tag.
   - **×** dismisses it.
   - **✓ All** applies every suggestion at once.

Suggestions are stored with the recording, so they're still waiting for you
after a restart.

## On demand

The **✨ Suggest** button in any recording's tag row runs the same step
immediately — even when the automatic pipeline step is turned off. Useful for
tagging older recordings after you enable the feature.

## Settings

**Settings → Post-Processing → Auto-Tagging**:

| Setting | Meaning |
|---------|---------|
| Suggest tags automatically | Run the step on every new recording |
| Provider | `Same as post-processing` (default) reuses your cleanup connection; or pick Ollama / OpenAI-compatible / Groq / Anthropic |
| API key / URL | Only for cloud providers; blank inherits the cleanup values |
| Model | Blank = the cleanup model |
| Max suggestions | 1–12 (default 5) |
| Instructions | The prompt that steers the tagger — your tag list and the transcript are appended automatically |

The auto-tag model also has its own tab in the **Models modal** (header →
Quick model switch, or `r` on a recording), alongside transcription, cleanup,
summary, live preview, and the semantic embedding model.

## Config (`config.toml`)

```toml
[auto_tag]
auto = true            # suggest on every new recording
provider = ""          # "" = inherit [llm_post_process]
api_key = ""           # "" = inherit
api_url = ""           # "" = inherit / provider default
model = ""             # "" = the cleanup model
max_tags = 5
prompt = "You tag voice-note transcripts. …"
```

## Privacy note

Auto-tagging sends the transcript to whichever provider you configure — with
**Local Ollama** everything stays on your machine, exactly like Smart Cleanup.

## Auto-applying existing tags

With **Settings → Post-Processing → Auto-Tagging → Auto-apply existing tags**
on, a suggestion that matches a tag you already have (say the model suggests
`code` and a `code` tag exists) is applied immediately — no chip, no click.
Only suggestions that would **create a brand-new tag** wait for your approval.
Matching is case-insensitive and considers every tag in your library, attached
or not.

The suggestion chips also have **✓ All** / **✕ All** buttons to apply or
dismiss everything at once *on that one recording*.

## Clearing suggestions in bulk

Decided to stop using auto-tagging, or just want a clean slate? **Settings →
Post-Processing → Auto-Tagging → 🧹 Clear all suggestions** drops every pending
✨ chip across your *entire* library in one sweep (after a confirm). Tags you
already approved stay attached — only the not-yet-decided proposals go.

The same library-wide sweep is on the CLI:

```bash
phoneme tag clear-suggestions
```

To review or decide a single recording's suggestions from the CLI instead:

```bash
phoneme tag suggestions <recording-id>                 # list them
phoneme tag suggestions <recording-id> --approve code  # create+attach "code"
phoneme tag suggestions <recording-id> --dismiss code  # drop the proposal
```
