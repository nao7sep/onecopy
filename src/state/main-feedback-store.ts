import { create } from "zustand";
import type { Status } from "../models/status";

const scopes = {
  comparison: "selection",
  viewer: "selection",
  playback: "selection",
  selection: "selection",
  detail: "selection",
  recheck: "section",
} as const;
type Owner = keyof typeof scopes;
interface Entry { request: number; result: Status | null }
interface FeedbackState { entries: Partial<Record<Owner, Entry>> }

/** Only Main's bounded command feedback lives here. Durable errors and file
 * operation receipts retain their own existing presentation and recovery. */
export const useMainFeedbackStore = create<FeedbackState>(() => ({ entries: {} }));
let nextRequest = 0;

export function beginMainFeedback(owner: Owner) {
  const request = ++nextRequest;
  useMainFeedbackStore.setState(({ entries }) => ({
    entries: { ...entries, [owner]: { request, result: null } },
  }));
  const current = () => useMainFeedbackStore.getState().entries[owner]?.request === request;
  return {
    current,
    finish(result: Status | null = null) {
      if (!current()) return;
      useMainFeedbackStore.setState(({ entries }) => ({
        entries: { ...entries, [owner]: { request, result } },
      }));
    },
  };
}

/** A section transition invalidates its selection too; mere background
 * refresh does neither unless reconciliation actually changes membership. */
export function invalidateMainFeedback(scope: "selection" | "section"): void {
  useMainFeedbackStore.setState(({ entries }) => ({
    entries: Object.fromEntries(Object.entries(entries).filter(([owner]) =>
      scope === "selection" && scopes[owner as Owner] === "section",
    )),
  }));
}

export function currentMainFeedback(state: FeedbackState): Status | null {
  const rank = { danger: 2, warning: 1, normal: 0 };
  const entries = Object.values(state.entries).filter((entry) => entry.result !== null);
  entries.sort((left, right) =>
    rank[right.result!.tone] - rank[left.result!.tone] || right.request - left.request,
  );
  return entries[0]?.result ?? null;
}
