// The derived-work coordinator's frontend duties: report recent user input
// for idle-only classes, and narrate the fixed work classes in the status bar.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { log, toErrorFields } from "../repositories";

const PING_EVERY_MS = 10_000;

interface DerivedWorkState {
  /** The status-bar line; null hides it. */
  line: string | null;
}

export const useDerivedWorkStore = create<DerivedWorkState>(() => ({ line: null }));

const CLASS_LABELS: Record<string, string> = {
  previews: "previews",
  "video-posters": "video posters",
  similarity: "similar photos",
  strips: "video snapshots",
  transcripts: "transcripts",
  faces: "face scores",
};

let lastPing = 0;
function ping(): void {
  const now = Date.now();
  if (now - lastPing < PING_EVERY_MS) return;
  lastPing = now;
  void invoke("note_user_activity").catch((error) =>
    log.warn("activity ping failed", toErrorFields(error)),
  );
}

export function installActivityPings(target: Window): void {
  // Capture phase so no surface can swallow the signal before it counts.
  for (const event of ["keydown", "pointerdown", "wheel"] as const) {
    target.addEventListener(event, ping, { capture: true, passive: true });
  }
}

void (async () => {
  try {
    await listen<{ class: string; done?: number; total?: number }>(
      "derived://progress",
      (event) => {
        const label = CLASS_LABELS[event.payload.class] ?? event.payload.class;
        const counts =
          typeof event.payload.done === "number" && typeof event.payload.total === "number"
            ? ` ${event.payload.done}/${event.payload.total}`
            : "…";
        useDerivedWorkStore.setState({ line: `Filling in: ${label}${counts}` });
      },
    );
    await listen("derived://quiet", () => {
      useDerivedWorkStore.setState({ line: null });
    });
  } catch (error) {
    log.warn("derived-work event wiring failed", toErrorFields(error));
  }
})();
