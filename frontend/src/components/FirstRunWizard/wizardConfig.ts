// Pure config helpers for the first-run wizard — hardware-aware defaults, the
// "what we'll install" plan, scratch-key stripping, and the preview_whisper
// builders. None of these touch component state or trigger renders; the wizard
// passes its `config` (mutated by reference, same as before) plus the detected
// RAM/VRAM, and calls requestUpdate() itself. Lifted out of index.ts verbatim.

/** Pre-select the locally-recommended features + models for the detected
 *  hardware (idempotent — only fills choices that aren't already set). Shared
 *  by the express path and the customize feature picker. Mutates `config`. */
export function applyRecommendedSetup(config: any, systemRamMb: number, systemVramMb: number) {
  if (config._setup_whisper === undefined) {
    if (systemRamMb >= 16000 || systemVramMb >= 6000) {
      config._setup_whisper = true;
      config._setup_ollama = true;
      config.semantic_search = { enabled: true };
      config._setup_diarization = true;
      config._setup_native_streaming = true;
    } else if (systemRamMb >= 8000 || systemVramMb >= 4000) {
      config._setup_whisper = true;
      config._setup_ollama = false;
      config.semantic_search = { enabled: true };
      config._setup_diarization = false;
      config._setup_native_streaming = false;
    } else {
      config._setup_whisper = true;
      config._setup_ollama = false;
      config.semantic_search = { enabled: false };
      config._setup_diarization = false;
      config._setup_native_streaming = false;
    }
  }
  if (!config._whisper_model_choice) {
    if (systemRamMb >= 32000 || systemVramMb >= 8000) config._whisper_model_choice = "ggml-large-v3-turbo-q5_0.bin";
    else if (systemRamMb >= 16000 || systemVramMb >= 4000) config._whisper_model_choice = "ggml-medium.en.bin";
    else if (systemRamMb >= 8000 || systemVramMb >= 2000) config._whisper_model_choice = "ggml-small.en.bin";
    else config._whisper_model_choice = "ggml-base.en.bin";
  }
  if (!config._ollama_model_choice) {
    if (systemRamMb >= 64000 || systemVramMb >= 24000) config._ollama_model_choice = "llama3.3:70b";
    else if (systemRamMb >= 32000 || systemVramMb >= 16000) config._ollama_model_choice = "qwen2.5:32b";
    else if (systemRamMb >= 16000 || systemVramMb >= 6000) config._ollama_model_choice = "llama3.1:8b";
    else config._ollama_model_choice = "llama3.2:3b";
  }
}

/** Human "what will be installed" plan for the detected hardware, used by the
 *  express welcome's summary (and as the model labels). */
export function recommendedPlan(config: any): { icon: string; title: string; detail: string }[] {
  const WHISPER_LABELS: Record<string, string> = {
    "ggml-large-v3-turbo-q5_0.bin": "Whisper Large v3 Turbo (~1.1 GB)",
    "ggml-large-v3.bin": "Whisper Large v3 (~3.1 GB)",
    "ggml-medium.en.bin": "Whisper Medium (~1.5 GB)",
    "ggml-small.en.bin": "Whisper Small (~480 MB)",
    "ggml-base.en.bin": "Whisper Base (~140 MB)",
  };
  const plan: { icon: string; title: string; detail: string }[] = [
    { icon: "🎙️", title: "Speech-to-text engine", detail: `whisper.cpp + ${WHISPER_LABELS[config._whisper_model_choice] ?? "a Whisper model"}` },
  ];
  if (config._setup_ollama) {
    plan.push({ icon: "✨", title: "Local AI (cleanup + summaries)", detail: `Ollama + ${config._ollama_model_choice}` });
  }
  if (config.semantic_search?.enabled) {
    plan.push({ icon: "🔍", title: "Semantic search", detail: "all-MiniLM embedding model (~90 MB)" });
  }
  if (config._setup_diarization) {
    plan.push({ icon: "🗣️", title: "Speaker labels", detail: "speakrs diarization models (~500 MB)" });
  }
  return plan;
}

/** Strip the wizard's scratch keys, returning a clean copy safe to write to
 *  disk. The `_setup_*` / `_*_choice` keys are UI-only and must never persist. */
export function stripScratchKeys(config: any): any {
  const cleanConfig = { ...config };
  delete cleanConfig._setup_whisper;
  delete cleanConfig._setup_ollama;
  delete cleanConfig._setup_diarization;
  delete cleanConfig._whisper_model_choice;
  delete cleanConfig._ollama_model_choice;
  delete cleanConfig._setup_native_streaming;
  delete cleanConfig._setup_preview;
  return cleanConfig;
}

/** Build a dedicated-local preview_whisper block from the main whisper config so
 *  every required field is present. `previewPort` is the distinct second port. */
export function buildPreviewLocal(whisper: any, modelPath: string, previewPort: number): any {
  return {
    ...whisper,
    provider: "local",
    mode: "bundled_model",
    model_path: modelPath,
    // Distinct port from the final server so both run concurrently.
    bundled_server_port: previewPort,
    api_key: "",
  };
}

/** Build a cloud-API preview_whisper block, carrying over any key/model/url the
 *  user already typed (`existing` is the current preview_whisper, may be empty). */
export function buildPreviewApi(whisper: any, provider: string, existing: any): any {
  return {
    ...whisper,
    provider,
    mode: "external",
    model_path: "",
    api_key: existing.api_key ?? "",
    model: existing.model ?? "",
    api_url: existing.api_url ?? "",
  };
}
