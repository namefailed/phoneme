# Smart Cleanup (LLM Post-Processing)

Phoneme includes a built-in **Smart Cleanup** feature.

Instead of just getting raw, sometimes imperfect Whisper transcriptions, Phoneme can pipe your transcript through a Large Language Model (LLM) before saving it. This allows you to automatically remove stutters, fix phonetic misunderstandings, translate languages on-the-fly, or format your spoken thoughts into clean bullet points.

## How it works

When Smart Cleanup is enabled, the pipeline looks like this:
1. You finish speaking.
2. Whisper transcribes the audio.
3. **The LLM takes the raw transcript, follows your exact Prompt instructions, and rewrites it.**
4. The finalized text is saved to your database and sent to your Hooks.

## Setting up Local AI (Free & Offline)

For the ultimate privacy-respecting, local-first experience, you can run the LLM locally on your own hardware using Ollama.

1. Download and install [Ollama](https://ollama.com/).
2. Open your terminal and run: `ollama run llama3.2:3b`. This will download a highly capable, fast, 3-billion parameter model.
3. In Phoneme's Settings -> **Smart Cleanup (AI)**:
   - Check **Enable Smart Cleanup**
   - **AI Provider**: `Local Ollama`
   - **Model Name**: `llama3.2:3b`
   - **API Key**: Leave blank.

## Prompts & Presets

The magic of the LLM is in the prompt. You can select one of our default presets, or write your own. The single most important rule when writing a custom prompt is:

> [!WARNING]
> You **must** instruct the AI to reply ONLY with the final text. Otherwise, the AI might add conversational filler like *"Here is your cleaned transcript:"* which will ruin your notes!

### Useful Prompt Ideas

**The Dysfluency Fixer**
> I have a speech impediment that causes me to stutter and repeat sounds. Carefully clean up the transcript so it flows perfectly, removing any dysfluency while preserving my intended meaning. Reply ONLY with the cleaned text.

**The Executive Assistant**
> Format this raw transcript into a clean, professional meeting note. Use bullet points or headings if appropriate. Output ONLY the formatted notes and absolutely no conversational filler.

**The Universal Translator**
> Translate this transcript into perfect English. Keep the meaning exact and natural. Output ONLY the English translation and absolutely nothing else.

## Using OpenAI or Compatible APIs

If you don't have the hardware to run Ollama smoothly, you can use OpenAI (or any OpenAI-compatible API like Groq or TogetherAI).

1. Select **OpenAI-Compatible Endpoint** as your provider.
2. Enter the Model Name (e.g., `gpt-4o-mini`).
3. Enter your API Key.

Enjoy perfectly polished transcripts!
