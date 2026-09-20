// One frontend projection of the coordinator snapshot. Output facts remain
// the queue in Rust; this store only presents current lifecycle and sends
// pause/resume intent back to that single owner.

import { invoke } from "@tauri-apps/api/core";
import { workReasonKey } from "../models/workReasons";
import { create } from "zustand";
import type { MessageKey } from "../i18n/catalogues";
import { message, type Message, type Translator } from "../i18n/translate";
import { log, toErrorFields } from "../repositories";
import { requestSeq } from "./request-seq";
import { recordActionFailure } from "./notifications-store";
import type { ItemWorkStates } from "../models/items";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { createEventInstaller } from "../utils/eventInstallation";
import { useSectionsStore } from "./sections-store";
import {
  finishActivityOperation,
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
  workerRunning: boolean;
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
  workerRunning: boolean;
  pausedClasses: BackgroundClassSnapshot["id"][];
  active: ActiveItemWork | null;
}

interface DerivedWorkState {
  snapshot: BackgroundWorkSnapshot | null;
  loading: boolean;
  changing: string | null;
  error: Message | null;
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
      set({ loading: false, error: message("work.loadFailed") });
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
      set({ error: message("work.changeFailed") });
      log.warn("background-work pause failed", toErrorFields(error));
      recordActionFailure(
        "background-work-control-failed",
        message("work.controlFailed"),
        error,
      );
    } finally {
      set((state) => ({
        changing: state.changing === changing ? null : state.changing,
      }));
    }
  },
}));

const CLASS_LABELS: Record<BackgroundClassSnapshot["id"], MessageKey> = {
  previews: "work.classPreviews",
  snapshots: "wizard.videoSnapshots",
  similarity: "work.classSimilarity",
  faces: "wizard.faceScoring",
  "video-transcripts": "wizard.videoTranscription",
  "audio-transcripts": "wizard.audioTranscription",
};

/** One whole sentence per class rather than "Stopping" plus a lowercased
 * label: a language that capitalizes its nouns, or puts the verb last, cannot
 * be served by lowercasing a label the catalogue already holds. */
const STOPPING_LINES: Record<BackgroundClassSnapshot["id"], MessageKey> = {
  previews: "work.stoppingPreviews",
  snapshots: "work.stoppingSnapshots",
  similarity: "work.stoppingSimilarity",
  faces: "work.stoppingFaces",
  "video-transcripts": "work.stoppingVideoTranscripts",
  "audio-transcripts": "work.stoppingAudioTranscripts",
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

export function backgroundClassLabel(id: BackgroundClassSnapshot["id"]): MessageKey {
  return CLASS_LABELS[id];
}

/** The chip's one line. Which fact wins depends on the live snapshot, and a
 * waiting row may carry a reason the core recorded, so this resolves words
 * here rather than returning a descriptor. */
export function backgroundWorkLine(
  snapshot: BackgroundWorkSnapshot | null,
  t: Translator["t"],
): string {
  if (snapshot === null) return t("work.title");
  const rows = backgroundRows(snapshot);
  const stopping = rows.find((row) => row.state === "stopping");
  if (stopping) return t(STOPPING_LINES[stopping.id]);
  const running = rows.find((row) => row.state === "running");
  if (running) {
    const name = t(backgroundClassLabel(running.id));
    return running.done !== null && running.total !== null
      ? t("work.classProgress", { name, done: running.done, total: running.total })
      : t("work.classRunning", { name });
  }
  if (!snapshot.workerRunning) return t("work.enrichmentStopped");
  if (rows.some((row) => row.state === "paused") && rows.every((row) => row.state === "paused" || row.state === "disabled")) {
    return t("work.allPaused");
  }
  const queued = rows.find((row) => row.state === "queued" && row.queued > 0);
  if (queued) {
    return t("work.classQueued", {
      name: t(backgroundClassLabel(queued.id)),
      count: queued.queued,
    });
  }
  if (rows.some((row) => row.state === "paused")) return t("work.somePaused");
  const waiting = rows.find((row) => row.state === "waiting" || row.state === "unavailable");
  // A waiting row's reason is recorded by the core; it shows as it arrived.
  if (waiting) {
    const key = workReasonKey(waiting.reason);
    return key === null ? (waiting.reason ?? t("work.waiting")) : t(key);
  }
  return t("work.noWorkRunning");
}

const ITEM_CLASS_FIELD: Record<ActiveItemWork["id"], keyof ItemWorkStates> = {
  previews: "preview",
  snapshots: "snapshots",
  similarity: "similarity",
  faces: "faces",
  "video-transcripts": "transcripts",
  "audio-transcripts": "transcripts",
};


/** Overlays the live runtime on one item's stored work facts. The overlay's own
 * reason is the app's word, so it arrives translated; the core's recorded
 * reasons keep theirs. */
export function mergeActiveItemWork(
  states: ItemWorkStates,
  hash: string | null,
  active: ActiveItemWork | null,
  t: Translator["t"],
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
      reason: active.stopping ? t("activity.stateStopping") : null,
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
    workerRunning: runtime.workerRunning,
    pausedClasses: runtime.pausedClasses,
    activeItem: runtime.active,
  };
}

/** Resume a stopped coordinator without changing other rows' pause choices. */
export function backgroundRowCanResume(snapshot: BackgroundWorkSnapshot, row: BackgroundClassSnapshot): boolean {
  return row.state !== "disabled" && (row.state === "paused" || row.state === "stopping" ||
    (!snapshot.workerRunning && snapshot.activeItem?.id !== row.id));
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

const installEvents = createEventInstaller(
  async (listeners) => {
    await listeners.listen<BackgroundRuntimeSnapshot>("derived://state-changed", (event) => {
      runtimeVersion += 1;
      latestRuntime = event.payload;
      useDerivedWorkStore.setState((state) => ({
        snapshot: mergeBackgroundRuntime(state.snapshot, event.payload),
        activeItem: event.payload.active,
      }));
    });
    await listeners.listen("derived://quiet", () => {
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
    recordInterfaceFailure(message("work.liveStatusUnavailable"));
    useDerivedWorkStore.setState({ error: message("work.liveStatusUnavailable") });
  },
);

/** Installs the app-lifetime projection only after startup has admitted the
 * feature application. Importing this module must never contact the backend. */
export function installDerivedWorkEventWiring(): Promise<void> {
  return installEvents();
}
