/**
 * Pure grouping logic for the recordings list (v1.6 Session Grouping).
 *
 * Meeting Mode records two tracks (microphone + system audio) that share a
 * non-null `session_id`. In the list we want to show those two rows as a single
 * collapsible group instead of two flat rows. Standalone recordings (null
 * `session_id`) stay exactly as they were.
 *
 * This module is deliberately DOM-free so the grouping behaviour can be
 * unit-tested without a renderer.
 */
import type { Recording } from "../../services/ipc";

/** A single standalone recording (no meeting session). */
export type SingleItem = { kind: "single"; recording: Recording };

/** A meeting: two-or-more recordings sharing one `session_id`. */
export type GroupItem = {
  kind: "group";
  sessionId: string;
  tracks: Recording[];
};

export type GroupedItem = SingleItem | GroupItem;

/**
 * Group recordings that share a `session_id` into one `GroupItem`, preserving
 * the input order. Standalone recordings (null/empty `session_id`) become
 * `SingleItem`s in place.
 *
 * Order is preserved by first appearance of each session: the group lands where
 * its first member appeared in the input. Members within a group keep their
 * input order. The backend already returns the two tracks adjacent (they share
 * a start time), so in practice a group is contiguous; collecting by session id
 * rather than only-consecutive runs makes this robust even if an unrelated row
 * ever slips between the two tracks.
 *
 * A session with only ONE recording present (e.g. the other track was deleted)
 * is rendered as a `SingleItem`, not a one-member group — there is nothing to
 * collapse.
 */
export function groupRecordings(recordings: Recording[]): GroupedItem[] {
  const items: GroupedItem[] = [];
  // Map each session id to the index in `items` where its group lives.
  const groupIndex = new Map<string, number>();

  for (const rec of recordings) {
    const sid = rec.session_id;
    if (!sid) {
      items.push({ kind: "single", recording: rec });
      continue;
    }
    const existing = groupIndex.get(sid);
    if (existing === undefined) {
      groupIndex.set(sid, items.length);
      items.push({ kind: "group", sessionId: sid, tracks: [rec] });
    } else {
      (items[existing] as GroupItem).tracks.push(rec);
    }
  }

  // Demote single-member "groups" to singles — a lone track is not a meeting.
  return items.map((item) => {
    if (item.kind === "group" && item.tracks.length === 1) {
      return { kind: "single", recording: item.tracks[0] };
    }
    return item;
  });
}

/**
 * Flatten grouped items into the list of recordings that are actually VISIBLE
 * as rows, given which groups are expanded. The order matches the rendered DOM
 * row order exactly, so callers can use array index <-> `.rec-row` index
 * interchangeably for keyboard navigation and range selection.
 *
 * A collapsed group contributes no member rows (only its header, which is not a
 * `.rec-row`). An expanded group contributes its member rows in order.
 */
export function visibleRecordings(
  items: GroupedItem[],
  isExpanded: (sessionId: string) => boolean,
): Recording[] {
  const out: Recording[] = [];
  for (const item of items) {
    if (item.kind === "single") {
      out.push(item.recording);
    } else if (isExpanded(item.sessionId)) {
      out.push(...item.tracks);
    }
  }
  return out;
}

/** Human label for a meeting track value ("mic" / "system"). */
export function trackLabel(track: string | null | undefined): string {
  switch (track) {
    case "mic":
      return "Microphone";
    case "system":
      return "System audio";
    default:
      return track ? track : "Track";
  }
}
