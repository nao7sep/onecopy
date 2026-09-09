// One frontend projection of the coordinator snapshot. Output facts remain
// the queue in Rust; this store only presents current lifecycle and sends
// pause/resume intent back to that single owner.

import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { log, toErrorFields } from "../repositories";
import { requestSeq } from "./request-seq";
import { recordActionFailure } from "./notifications-store";
import type { ItemWorkStates } from "../models/items";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { createEventInstaller } from "../utils/eventInstallation";
import { useSectionsStore } from "./sections-store";
import {
  finishActivityOperation,
  latestActivityOperationId,
  newActivityOperationId,
  recordActivity,
  type ActivityDraft,
} from "../repositories/activity";

const PING_EVERY_MS = 10_000;

export type BackgroundClassState =
  "disabled" | "unavailable" | "queued" | "waiting" | "running" | "stopping" | "paused" | "failed" | "up-to-date";

export interface BackgroundClassSnapshot {
  id: "previews" | "snapshots" | "similarity" | "faces" | "video-transcripts" | "audio-transcripts";
  state: BackgroundClassState;
  queued: number;
  failed: number;
  done: number | null;
  total: number | null;
  reason: string | null;
}

export interface BackgroundWorkSnapshot {
  pausedClasses: BackgroundClassSnapshot["id"][];
  activeItem: ActiveItemWork | null;
  classes: BackgroundClassSnapshot[];
}

export interface ActiveItemWork {
  id: BackgroundClassSnapshot["id"];
  hash: string | null;
  done: number | null;
  total: number | null;
  stopping: boolean;
}

interface BackgroundRuntimeSnapshot {
  pausedClasses: BackgroundClassSnapshot["id"][];
  active: ActiveItemWork | null;
}

interface DerivedWorkState {
  snapshot: BackgroundWorkSnapshot | null;
  loading: boolean;
  changing: string | null;
  error: string | null;
  activeItem: ActiveItemWork | null;
  load: () => Promise<void>;
  setPaused: (classId: string | null, paused: boolean) => Promise<void>;
}

const loadSequence = requestSeq();
let runtimeVersion = 0;
let latestRuntime: BackgroundRuntimeSnapshot | null = null;
let refreshTimer: ReturnType<typeof setTimeout> | null = null;

/** Coalesce output invalidations; a large library must never accumulate snapshot reads. */
export function refreshBackgroundWorkSoon(): void {
  if (refreshTimer !== null) return;
  refreshTimer = setTimeout(() => {
    refreshTimer = null;
    if (useDerivedWorkStore.getState().loading) { refreshBackgroundWorkSoon(); return; }
    void useDerivedWorkStore.getState().load();
  }, 1000);
}

export const useDerivedWorkStore = create<DerivedWorkState>((set, get) => ({
  snapshot: null,
  loading: false,
  changing: null,
  error: null,
  activeItem: null,

  load: async () => {
    const fresh = loadSequence.begin();
    const version = runtimeVersion;
    set({ loading: true });
    try {
      const response = await invoke<BackgroundWorkSnapshot>("background_work_snapshot");
      if (fresh()) {
        const runtime = version !== runtimeVersion ? latestRuntime : null;
        set({
          snapshot: runtime === null ? response : mergeBackgroundRuntime(response, runtime),
          activeItem: runtime === null ? response.activeItem : runtime.active,
          loading: false, error: null,
        });
      }
    } catch (error) {
      if (!fresh()) return;
      set({
        loading: false,
        error: "Background work could not be loaded. Try reopening this window.",
      });
      log.warn("background-work snapshot failed", toErrorFields(error));
    }
  },

  setPaused: async (classId, paused) => {
    const changing = classId ?? "all";
    set({ changing, error: null });
    try {
      const operationId = newActivityOperationId("backgroundWork");
      recordActivity({
        kind: paused ? "paused" : "resumed",
        owner: "backgroundWork",
        subject:
          classId !== null && classId in ACTIVITY_SUBJECTS
            ? ACTIVITY_SUBJECTS[classId as BackgroundClassSnapshot["id"]]
            : undefined,
        operationId,
        current: paused ? "paused" : "running",
        reason: paused ? "pause" : "user",
      });
      finishActivityOperation("backgroundWork", operationId);
      await invoke("background_work_set_paused", { classId, paused });
      if (classId === null) await useSectionsStore.getState().loadIndexWork();
      await get().load();
    } catch (error) {
      set({ error: "Background work could not be changed. Try again." });
      log.warn("background-work pause failed", toErrorFields(error));
      recordActionFailure("background-work-control-failed", "Couldn’t change background work.", error);
    } finally {
      set((state) => ({
        changing: state.changing === changing ? null : state.changing,
      }));
    }
  },
}));

const CLASS_LABELS: Record<BackgroundClassSnapshot["id"], string> = {
  previews: "Thumbnails, previews, and posters",
  snapshots: "Video snapshots",
  similarity: "Similar photos",
  faces: "Face scoring",
  "video-transcripts": "Video transcription",
  "audio-transcripts": "Audio transcription",
};

const ACTIVITY_SUBJECTS: Record<
  BackgroundClassSnapshot["id"],
  NonNullable<ActivityDraft["subject"]>
> = {
  previews: "previews",
  snapshots: "snapshots",
  similarity: "similarity",
  faces: "faces",
  "video-transcripts": "videoTranscription",
  "audio-transcripts": "audioTranscription",
};

export function backgroundClassLabel(id: BackgroundClassSnapshot["id"]): string {
  return CLASS_LABELS[id];
}

export function backgroundWorkLine(snapshot: BackgroundWorkSnapshot | null): string {
  if (snapshot === null) return "Background work";
  const rows = backgroundRows(snapshot);
  const stopping = rows.find((row) => row.state === "stopping");
  if (stopping) return `Stopping ${backgroundClassLabel(stopping.id).toLowerCase()}…`;
  const running = rows.find((row) => row.state === "running");
  if (running) {
    const progress = running.done !== null && running.total !== null ? ` ${running.done}/${running.total}` : "…";
    return `${backgroundClassLabel(running.id)}${progress}`;
  }
  if (rows.some((row) => row.state === "paused") && rows.every((row) => row.state === "paused" || row.state === "disabled")) {
    return "Background work paused";
  }
  const queued = rows.find((row) => row.state === "queued" && row.queued > 0);
  if (queued) return `${backgroundClassLabel(queued.id)}: ${queued.queued} queued`;
  if (rows.some((row) => row.state === "paused")) return "Some background work paused";
  const waiting = rows.find((row) => row.state === "waiting" || row.state === "unavailable");
  if (waiting) return waiting.reason ?? "Background work waiting";
  return "Background work: no work running";
}

const ITEM_CLASS_FIELD: Record<ActiveItemWork["id"], keyof ItemWorkStates> = {
  previews: "preview",
  snapshots: "snapshots",
  similarity: "similarity",
  faces: "faces",
  "video-transcripts": "transcripts",
  "audio-transcripts": "transcripts",
};

type ProjectedActiveWork = Pick<ActiveItemWork, "id" | "done" | "total" | "stopping"> & {
  operationId: string;
};

/**
 * Turns the coordinator's high-frequency runtime projection into lifecycle
 * evidence. A momentary `active: null` is an implementation pulse between
 * items, not proof that the class is finished; `derived://quiet` is the
 * authoritative settled boundary.
 */
export class BackgroundActivityProjection {
  private active: ProjectedActiveWork | null = null;

  observe(runtime: BackgroundRuntimeSnapshot, causeId?: string): ActivityDraft[] {
    const next = runtime.active;
    if (next === null) return [];

    const events: ActivityDraft[] = [];
    if (this.active !== null && this.active.id !== next.id) {
      events.push(this.completed(this.active.id, this.active.operationId, causeId));
      finishActivityOperation("backgroundWork", this.active.operationId);
      this.active = null;
    }

    if (this.active === null) {
      const operationId = newActivityOperationId("backgroundWork");
      events.push({
        kind: next.stopping ? "stopping" : "started",
        owner: "backgroundWork",
        subject: ACTIVITY_SUBJECTS[next.id],
        operationId,
        causeId,
        current: next.stopping ? "stopping" : "running",
        reason: next.stopping ? "preemption" : undefined,
        done: next.done ?? undefined,
        total: next.total ?? undefined,
      });
      this.active = {
        id: next.id,
        done: next.done,
        total: next.total,
        stopping: next.stopping,
        operationId,
      };
    } else if (
      this.active.stopping !== next.stopping ||
      this.active.done !== next.done ||
      this.active.total !== next.total
    ) {
      events.push({
        kind: next.stopping ? "stopping" : "progressed",
        owner: "backgroundWork",
        subject: ACTIVITY_SUBJECTS[next.id],
        operationId: this.active.operationId,
        causeId,
        current: next.stopping ? "stopping" : "running",
        reason: next.stopping ? "preemption" : undefined,
        done: next.done ?? undefined,
        total: next.total ?? undefined,
      });
    }

    if (this.active !== null) {
      this.active = {
        ...this.active,
        id: next.id,
        done: next.done,
        total: next.total,
        stopping: next.stopping,
      };
    }
    return events;
  }

  quiet(causeId?: string): ActivityDraft[] {
    if (this.active === null) return [];
    const event = this.completed(this.active.id, this.active.operationId, causeId);
    finishActivityOperation("backgroundWork", this.active.operationId);
    this.active = null;
    return [event];
  }

  private completed(
    id: ActiveItemWork["id"],
    operationId: string,
    causeId?: string,
  ): ActivityDraft {
    return {
      kind: "completed",
      owner: "backgroundWork",
      subject: ACTIVITY_SUBJECTS[id],
      operationId,
      causeId,
      current: "idle",
      reason: "completion",
    };
  }
}

export function mergeActiveItemWork(
  states: ItemWorkStates,
  hash: string | null,
  active: ActiveItemWork | null,
): ItemWorkStates {
  if (hash === null || active?.hash !== hash) return states;
  const field = ITEM_CLASS_FIELD[active.id];
  const current = states[field];
  if (current === null) return states;
  return {
    ...states,
    [field]: {
      ...current,
      state: "running",
      reason: active.stopping ? "Stopping" : null,
      done: active.done,
      total: active.total,
    },
  };
}

export function mergeBackgroundRuntime(
  snapshot: BackgroundWorkSnapshot | null,
  runtime: BackgroundRuntimeSnapshot,
): BackgroundWorkSnapshot | null {
  if (snapshot === null) return null;
  return {
    ...snapshot,
    pausedClasses: runtime.pausedClasses,
    activeItem: runtime.active,
  };
}

/** Runtime is a reversible overlay; database-authored availability never gets overwritten. */
export function backgroundRows(snapshot: BackgroundWorkSnapshot): BackgroundClassSnapshot[] {
  return snapshot.classes.map((row) => {
    const isPaused = snapshot.pausedClasses.includes(row.id);
    const active = snapshot.activeItem;
    if (active?.id === row.id) {
      return {
        ...row,
        state: isPaused || active.stopping ? "stopping" : "running",
        done: active.done,
        total: active.total,
      };
    }
    if (isPaused && row.state !== "disabled") return { ...row, state: "paused", done: null, total: null };
    return row;
  });
}

let lastPing = 0;
function ping(): void {
  const now = Date.now();
  if (now - lastPing < PING_EVERY_MS) return;
  lastPing = now;
  void invoke("note_user_activity").catch((error) => log.warn("activity ping failed", toErrorFields(error)));
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

const backgroundActivity = new BackgroundActivityProjection();

const installEvents = createEventInstaller(
  async (listeners) => {
    await listeners.listen<BackgroundRuntimeSnapshot>("derived://state-changed", (event) => {
      runtimeVersion += 1;
      latestRuntime = event.payload;
      useDerivedWorkStore.setState((state) => ({
        snapshot: mergeBackgroundRuntime(state.snapshot, event.payload),
        activeItem: event.payload.active,
      }));
      for (const draft of backgroundActivity.observe(event.payload, latestActivityOperationId("priority"))) {
        recordActivity(draft);
      }
    });
    await listeners.listen("derived://quiet", () => {
      for (const draft of backgroundActivity.quiet(latestActivityOperationId("priority"))) {
        recordActivity(draft);
      }
      refreshBackgroundWorkSoon();
    });
    for (const event of [
      "source-check://progress", "source-check://done", "file-information://progress",
      "file-information://done", "watch://updated", "derived://issues",
      "derived://similarity-updated", "binaries://changed",
    ]) await listeners.listen(event, refreshBackgroundWorkSoon);
    await useDerivedWorkStore.getState().load();
  },
  (error) => {
    log.warn("derived-work event wiring failed", toErrorFields(error));
    recordInterfaceFailure("Live previews-and-analysis status is unavailable. Restart OneCopy to repair it.");
    useDerivedWorkStore.setState({
      error: "Live previews-and-analysis status is unavailable. Restart OneCopy to repair it.",
    });
  },
);

/** Installs the app-lifetime projection only after startup has admitted the
 * feature application. Importing this module must never contact the backend. */
export function installDerivedWorkEventWiring(): Promise<void> {
  return installEvents();
}
