// One frontend projection of the coordinator snapshot. Output facts remain
// the queue in Rust; this store only presents current lifecycle and sends
// pause/resume intent back to that single owner.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { log, toErrorFields } from "../repositories";
import { requestSeq } from "./request-seq";

const PING_EVERY_MS = 10_000;

export type BackgroundClassState =
  | "disabled"
  | "unavailable"
  | "queued"
  | "waiting"
  | "running"
  | "stopping"
  | "paused"
  | "failed"
  | "up-to-date";

export interface BackgroundClassSnapshot {
  id: "previews" | "snapshots" | "similarity" | "faces" | "transcripts";
  state: BackgroundClassState;
  queued: number;
  failed: number;
  done: number | null;
  total: number | null;
  reason: string | null;
}

export interface BackgroundWorkSnapshot {
  masterPaused: boolean;
  classes: BackgroundClassSnapshot[];
}

interface BackgroundRuntimeSnapshot {
  masterPaused: boolean;
  pausedClasses: BackgroundClassSnapshot["id"][];
  active: {
    id: BackgroundClassSnapshot["id"];
    done: number | null;
    total: number | null;
    stopping: boolean;
  } | null;
}

interface DerivedWorkState {
  snapshot: BackgroundWorkSnapshot | null;
  open: boolean;
  loading: boolean;
  changing: string | null;
  error: string | null;
  load: () => Promise<void>;
  setOpen: (open: boolean) => void;
  setPaused: (classId: string | null, paused: boolean) => Promise<void>;
}

const loadSequence = requestSeq();

export const useDerivedWorkStore = create<DerivedWorkState>((set, get) => ({
  snapshot: null,
  open: false,
  loading: false,
  changing: null,
  error: null,

  load: async () => {
    const fresh = loadSequence.begin();
    set({ loading: true });
    try {
      const snapshot = await invoke<BackgroundWorkSnapshot>("background_work_snapshot");
      if (fresh()) set({ snapshot, loading: false, error: null });
    } catch (error) {
      if (fresh()) set({ loading: false, error: String(error) });
      log.warn("background-work snapshot failed", toErrorFields(error));
    }
  },

  setOpen: (open) => {
    set({ open });
    if (open) void get().load();
  },

  setPaused: async (classId, paused) => {
    const changing = classId ?? "all";
    set({ changing, error: null });
    try {
      await invoke("background_work_set_paused", { classId, paused });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
      log.warn("background-work pause failed", toErrorFields(error));
    } finally {
      set((state) => ({ changing: state.changing === changing ? null : state.changing }));
    }
  },
}));

const CLASS_LABELS: Record<BackgroundClassSnapshot["id"], string> = {
  previews: "Previews and posters",
  snapshots: "Video snapshots",
  similarity: "Similar photos",
  faces: "Face scoring",
  transcripts: "Transcription",
};

export function backgroundClassLabel(id: BackgroundClassSnapshot["id"]): string {
  return CLASS_LABELS[id];
}

export function backgroundWorkLine(snapshot: BackgroundWorkSnapshot | null): string {
  if (snapshot === null) return "Background work";
  const stopping = snapshot.classes.find((row) => row.state === "stopping");
  if (stopping) return `Stopping ${backgroundClassLabel(stopping.id).toLowerCase()}…`;
  const running = snapshot.classes.find((row) => row.state === "running");
  if (running) {
    const progress =
      running.done !== null && running.total !== null
        ? ` ${running.done}/${running.total}`
        : "…";
    return `${backgroundClassLabel(running.id)}${progress}`;
  }
  if (snapshot.masterPaused || snapshot.classes.some((row) => row.state === "paused")) {
    return "Background work paused";
  }
  if (snapshot.classes.some((row) => row.queued > 0)) return "Background work";
  return "Background work: up to date";
}

export function mergeBackgroundRuntime(
  snapshot: BackgroundWorkSnapshot | null,
  runtime: BackgroundRuntimeSnapshot,
): BackgroundWorkSnapshot | null {
  if (snapshot === null) return null;
  const paused = new Set(runtime.pausedClasses);
  return {
    masterPaused: runtime.masterPaused,
    classes: snapshot.classes.map((row) => {
      const isPaused = runtime.masterPaused || paused.has(row.id);
      if (runtime.active?.id === row.id) {
        return {
          ...row,
          state: isPaused || runtime.active.stopping ? "stopping" : "running",
          done: runtime.active.done,
          total: runtime.active.total,
        };
      }
      if (isPaused) return { ...row, state: "paused", done: null, total: null };
      if (row.state === "running" || row.state === "stopping") {
        return {
          ...row,
          state: row.queued > 0 ? "queued" : "up-to-date",
          done: null,
          total: null,
        };
      }
      return row;
    }),
  };
}

let lastPing = 0;
function ping(): void {
  const now = Date.now();
  if (now - lastPing < PING_EVERY_MS) return;
  lastPing = now;
  void invoke("note_user_activity").catch((error) =>
    log.warn("activity ping failed", toErrorFields(error)),
  );
}

export function installActivityPings(target: Window): () => void {
  // Capture phase so no surface can swallow the signal before it counts.
  for (const event of ["keydown", "pointerdown", "wheel"] as const) {
    target.addEventListener(event, ping, { capture: true, passive: true });
  }
  return () => {
    for (const event of ["keydown", "pointerdown", "wheel"] as const) {
      target.removeEventListener(event, ping, { capture: true });
    }
  };
}

void (async () => {
  try {
    await listen<BackgroundRuntimeSnapshot>("derived://state-changed", (event) => {
      useDerivedWorkStore.setState((state) => ({
        snapshot: mergeBackgroundRuntime(state.snapshot, event.payload),
      }));
    });
    await listen("derived://quiet", () => {
      if (useDerivedWorkStore.getState().open) {
        void useDerivedWorkStore.getState().load();
      }
    });
    await useDerivedWorkStore.getState().load();
  } catch (error) {
    log.warn("derived-work event wiring failed", toErrorFields(error));
  }
})();
