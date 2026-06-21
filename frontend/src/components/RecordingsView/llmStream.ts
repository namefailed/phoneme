/**
 * Pure accumulation logic for consuming the daemon's `llm_activity` stream into
 * a live display buffer, kept out of the Lit/detail components so it's
 * unit-testable without a DOM or event bus.
 *
 * The daemon streams every LLM stage in three phases (see the `LlmActivity` doc
 * in `crates/phoneme-ipc/src/schema.rs`): a prompt-start event (non-empty
 * `prompt`), then `delta` chunks, then a terminal `done`. A prompt-start marks a
 * fresh session — the buffer resets — and deltas append in order. The summary
 * peek and the meeting-digest card both tap the `summarizing` stage; this helper
 * gives them the same accumulation contract the AI-activity popout already uses
 * (`ThinkingPopout.ts`), including the delta-before-prompt ordering it handles.
 */
import type { DaemonEvent, PipelineStage } from "../../services/events";

/** The accumulating state of one streamed LLM stage. */
export type LlmStreamState = {
  /** Text accumulated so far (capped at the daemon's `MAX_STREAMED_CHARS`, so a
   *  long stage can be truncated here — the caller settles to the stored value). */
  text: string;
  /** True from prompt-start / first delta until the terminal `done`. */
  streaming: boolean;
};

/** A fresh, empty stream state (nothing received yet, not streaming). */
export function emptyLlmStream(): LlmStreamState {
  return { text: "", streaming: false };
}

/** Which streamed session a buffer cares about: a recording/track id + stage. */
export type LlmStreamFilter = { id: string; stage: PipelineStage };

/**
 * Does this event belong to the session `filter` is tracking? Only
 * `llm_activity` events for the matching id + stage do; everything else is
 * ignored so a buffer never picks up another recording's or stage's stream.
 */
export function matchesLlmStream(event: DaemonEvent, filter: LlmStreamFilter): boolean {
  return (
    event.event === "llm_activity" &&
    event.id === filter.id &&
    event.stage === filter.stage
  );
}

/**
 * Fold one matching `llm_activity` event into the buffer, returning the next
 * state (pure — never mutates `prev`). Caller must have already confirmed the
 * event matches via {@link matchesLlmStream}.
 *
 *  - A non-empty `prompt` starts a new session: reset the text and begin
 *    streaming (carrying any `delta` that rode the prompt-start event).
 *  - A `delta` without a prompt appends, opening the session if a delta arrives
 *    before the prompt (mirrors the popout's delta-before-prompt handling).
 *  - `done` ends the stream (clears the streaming flag) while keeping the text.
 */
export function applyLlmActivity(
  prev: LlmStreamState,
  event: Extract<DaemonEvent, { event: "llm_activity" }>,
): LlmStreamState {
  if (event.prompt) {
    // Prompt-start → fresh session. A regenerate's stream replaces the old one
    // rather than appending to it.
    return { text: event.delta ?? "", streaming: !event.done };
  }
  // Delta / done with no prompt: open the buffer on the first delta even if the
  // prompt hasn't arrived yet, then append.
  const text = event.delta ? prev.text + event.delta : prev.text;
  const streaming = event.done ? false : prev.streaming || !!event.delta;
  return { text, streaming };
}
