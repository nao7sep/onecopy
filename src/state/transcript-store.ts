// One transcript contract for every surface. The cache receipt in Rust owns
// pending/ready/empty/failed truth; this store adds only ephemeral loading,
// running percentage, and cancellation while forwarding the shared events.

import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { message, type Message } from "../i18n/translate";
import { log, toErrorFields } from "../repositories";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { createEventInstaller } from "../utils/eventInstallation";
import { recordActionFailure } from "./notifications-store";
import { recordActivity } from "../repositories/activity";

export type TranscriptStatus =
  "loading" | "pending" | "queued" | "running" | "ready" | "failed";

export interface TranscriptView {
  status: TranscriptStatus;
  text: string | null;
  /** The sentence the failed row shows. OneCopy's own words, so it follows a
   * language change; the core's recorded reason reaches the surface through
   * the work projection instead. */
  message: Message | null;
  percent: number | null;
  replacement: {
    status: "queued" | "running" | "failed";
    message: Message | null;
    percent: number | null;
  } | null;
  controlError?: Message | null;
}

interface TranscriptResult {
  status: "pending" | "ready" | "failed";
  text: string | null;
  message: string | null;
}

interface TranscriptState {
  rows: Record<string, TranscriptView>;
  load: (hash: string) => Promise<void>;
  start: (hash: string, replace?: boolean) => Promise<void>;
  cancel: () => Promise<void>;
}

const loading = new Set<string>();
const revisions = new Map<string, number>();
const recency = new Map<string, true>();
const MAX_CACHED_ROWS = 128;
let active: { hash: string; percent: number; replacement: boolean } | null = null;

const EMPTY: TranscriptView = {
  status: "loading",
  text: null,
  message: null,
  percent: null,
  replacement: null,
  controlError: null,
};

function patch(hash: string, value: Partial<TranscriptView>): void {
  useTranscriptStore.setState((state) => ({
    rows: {
      ...state.rows,
      [hash]: { ...(state.rows[hash] ?? EMPTY), ...value },
    },
  }));
}

function ensureRow(hash: string): void {
  recency.delete(hash);
  recency.set(hash, true);
  const current = useTranscriptStore.getState().rows;
  if (current[hash] !== undefined) return;
  const rows = { ...current, [hash]: EMPTY };
  while (recency.size > MAX_CACHED_ROWS) {
    const oldest = recency.keys().next().value as string | undefined;
    if (oldest === undefined) break;
    recency.delete(oldest);
    revisions.delete(oldest);
    delete rows[oldest];
  }
  useTranscriptStore.setState({ rows });
}

/** Marks an event or user action that a slower receipt read must not undo. */
function publish(hash: string, value: Partial<TranscriptView>): void {
  revisions.set(hash, (revisions.get(hash) ?? 0) + 1);
  patch(hash, value);
}

function publishIfLoaded(hash: string, value: Partial<TranscriptView>): void {
  if (useTranscriptStore.getState().rows[hash] !== undefined)
    publish(hash, value);
}

export const useTranscriptStore = create<TranscriptState>(() => ({
  rows: {},

  load: async (hash) => {
    if (loading.has(hash)) return;
    loading.add(hash);
    ensureRow(hash);
    const revision = revisions.get(hash) ?? 0;
    try {
      const result = await invoke<TranscriptResult>("transcript_get", { hash });
      if ((revisions.get(hash) ?? 0) !== revision) {
        recordActivity({
          kind: "stale",
          owner: "transcript",
          generation: revision,
          current: "stale",
          reason: "staleResponse",
        });
        return;
      }
      if (
        result.status === "ready" &&
        active?.hash === hash &&
        active.replacement
      ) {
        patch(hash, {
          status: "ready",
          text: result.text,
          message: null,
          percent: null,
          replacement: {
            status: "running",
            message: null,
            percent: active.percent,
          },
        });
      } else if (result.status === "pending" && active?.hash === hash) {
        patch(hash, {
          status: "running",
          text: null,
          message: null,
          percent: active.percent,
        });
      } else {
        // A non-failed receipt may carry a recorded reason; nothing shows it,
        // and the core reports it as a code in its own pass.
        patch(hash, {
          status: result.status,
          text: result.text,
          message:
            result.status === "failed" ? message("transcript.couldNotFinish") : null,
          percent: null,
        });
      }
    } catch (error) {
      if ((revisions.get(hash) ?? 0) !== revision) {
        recordActivity({
          kind: "stale",
          owner: "transcript",
          generation: revision,
          current: "stale",
          reason: "staleResponse",
        });
        return;
      }
      patch(hash, {
        status: "failed",
        message: message("transcript.loadFailed"),
        percent: null,
      });
      log.warn("transcript load failed", toErrorFields(error));
    } finally {
      loading.delete(hash);
    }
  },

  start: async (hash, replace = false) => {
    patch(hash, { controlError: null });
    if (replace) {
      publish(hash, {
        replacement: { status: "queued", message: null, percent: null },
      });
    } else {
      publish(hash, { status: "queued", message: null, percent: null });
    }
    const revision = revisions.get(hash) ?? 0;
    try {
      await invoke("transcribe", { hash, replace });
    } catch (error) {
      if ((revisions.get(hash) ?? 0) !== revision) {
        recordActivity({
          kind: "stale",
          owner: "transcript",
          generation: revision,
          current: "stale",
          reason: "staleResponse",
        });
        return;
      }
      if (replace) {
        publish(hash, {
          replacement: {
            status: "failed",
            message: message("transcript.replacementStartFailed"),
            percent: null,
          },
        });
      } else {
        publish(hash, {
          status: "failed",
          message: message("transcript.startFailed"),
          percent: null,
        });
      }
      log.error("transcribe start failed", toErrorFields(error));
      recordActionFailure(
        "transcription-start-failed",
        message("transcript.startFailedNotice"),
        error,
      );
    }
  },

  cancel: async () => {
    const target = active;
    try {
      await invoke("transcribe_cancel");
      if (target !== null) patch(target.hash, { controlError: null });
    } catch (error) {
      log.warn("transcription cancellation failed", toErrorFields(error));
      const failure = message("transcript.cancelFailed");
      if (target !== null) patch(target.hash, { controlError: failure });
      recordActionFailure("transcription-cancel-failed", failure, error);
    }
  },
}));

const installEvents = createEventInstaller(
  async (listeners) => {
    await listeners.listen<{
      hash: string;
      percent: number;
      replacement: boolean;
    }>(
      "transcribe://progress",
      (event) => {
        active = {
          hash: event.payload.hash,
          percent: event.payload.percent,
          replacement: event.payload.replacement,
        };
        const current = useTranscriptStore.getState().rows[event.payload.hash];
        if (
          event.payload.replacement ||
          (current?.replacement !== null &&
            current?.replacement !== undefined)
        ) {
          publishIfLoaded(event.payload.hash, {
            replacement: {
              status: "running",
              percent: event.payload.percent,
              message: null,
            },
          });
        } else {
          publishIfLoaded(event.payload.hash, {
            status: "running",
            percent: event.payload.percent,
            message: null,
          });
        }
      },
    );
    await listeners.listen<{ hash: string; text: string }>(
      "transcribe://done",
      (event) => {
        if (active?.hash === event.payload.hash) active = null;
        publishIfLoaded(event.payload.hash, {
          status: "ready",
          text: event.payload.text,
          message: null,
          percent: null,
          replacement: null,
        });
      },
    );
    await listeners.listen<{
      hash: string;
      message: string;
      replacement: boolean;
    }>(
      "transcribe://error",
      (event) => {
        log.error("transcription event reported failure", {
          hash: event.payload.hash,
          error: { message: event.payload.message },
        });
        if (active?.hash === event.payload.hash) active = null;
        const current = useTranscriptStore.getState().rows[event.payload.hash];
        if (
          event.payload.replacement ||
          (current?.replacement !== null &&
            current?.replacement !== undefined)
        ) {
          publishIfLoaded(event.payload.hash, {
            replacement: {
              status: "failed",
              message: message("transcript.couldNotFinish"),
              percent: null,
            },
          });
        } else {
          publishIfLoaded(event.payload.hash, {
            status: "failed",
            message: message("transcript.couldNotFinish"),
            percent: null,
          });
        }
      },
    );
    await listeners.listen<{ hash: string; replacement: boolean }>(
      "transcribe://cancelled",
      (event) => {
        if (active?.hash === event.payload.hash) active = null;
        const current = useTranscriptStore.getState().rows[event.payload.hash];
        if (
          event.payload.replacement ||
          (current?.replacement !== null &&
            current?.replacement !== undefined)
        ) {
          publishIfLoaded(event.payload.hash, { replacement: null });
        } else {
          publishIfLoaded(event.payload.hash, {
            status: "pending",
            message: null,
            percent: null,
          });
        }
      },
    );
  },
  (error) => {
    log.warn("transcript event wiring failed", toErrorFields(error));
    recordInterfaceFailure(message("transcript.liveUpdatesUnavailable"));
    const interrupted = active as { hash: string; percent: number } | null;
    if (interrupted !== null) {
      publishIfLoaded(interrupted.hash, {
        status: "failed",
        message: message("transcript.liveUpdatesUnavailable"),
        percent: null,
      });
      active = null;
    }
  },
);

/** Ready bootstrap owns app-lifetime transcript event admission. */
export function installTranscriptEventWiring(): Promise<void> {
  return installEvents();
}
