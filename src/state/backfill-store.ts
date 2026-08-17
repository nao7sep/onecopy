// The idle backfill's two frontend duties (Phase 33): tell the core when the
// user is here, and narrate what fills in while they are not.
//
// The activity ping is the scheduler's WHOLE view of the user — input events,
// throttled hard, because the signal only needs minute-level resolution (the
// core's idle threshold is 60 s) and a ping per keystroke would be an IPC
// storm in the exact keystroke-paced flow the scheduler exists to protect.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { log, toErrorFields } from "../repositories";

const PING_EVERY_MS = 10_000;

interface BackfillState {
  /** The status-bar chip line; null hides the chip. */
  line: string | null;
}

export const useBackfillStore = create<BackfillState>(() => ({ line: null }));

const CLASS_LABELS: Record<string, string> = {
  strips: "video snapshots",
  transcripts: "transcripts",
  faces: "face scores",
};

// ---- wiring, installed once at module load --------------------------------

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
      "backfill://progress",
      (event) => {
        const label = CLASS_LABELS[event.payload.class] ?? event.payload.class;
        const counts =
          typeof event.payload.done === "number" && typeof event.payload.total === "number"
            ? ` ${event.payload.done}/${event.payload.total}`
            : "…";
        useBackfillStore.setState({ line: `Filling in: ${label}${counts}` });
      },
    );
    await listen("backfill://quiet", () => {
      useBackfillStore.setState({ line: null });
    });
  } catch (error) {
    log.warn("backfill event wiring failed", toErrorFields(error));
  }
})();
