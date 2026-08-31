// One transcript contract for every surface. The cache receipt in Rust owns
// pending/ready/empty/failed truth; this store adds only ephemeral loading,
// running percentage, and cancellation while forwarding the shared events.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { log, toErrorFields } from "../repositories";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { reportActionFailure } from "./notifications-store";

export type TranscriptStatus =
  "loading" | "pending" | "queued" | "running" | "ready" | "failed";

export interface TranscriptView {
  status: TranscriptStatus;
  text: string | null;
  message: string | null;
  percent: number | null;
  replacement: {
    status: "queued" | "running" | "failed";
    message: string | null;
    percent: number | null;
  } | null;
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
let active: { hash: string; percent: number } | null = null;

const EMPTY: TranscriptView = {
  status: "loading",
  text: null,
  message: null,
  percent: null,
  replacement: null,
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
      if ((revisions.get(hash) ?? 0) !== revision) return;
      if (result.status === "pending" && active?.hash === hash) {
        patch(hash, {
          status: "running",
          text: null,
          message: null,
          percent: active.percent,
        });
      } else {
        patch(hash, {
          status: result.status,
          text: result.text,
          message: result.message,
          percent: null,
        });
      }
    } catch (error) {
      patch(hash, { status: "failed", message: String(error), percent: null });
      log.warn("transcript load failed", toErrorFields(error));
    } finally {
      loading.delete(hash);
    }
  },

  start: async (hash, replace = false) => {
    if (replace) {
      publish(hash, {
        replacement: { status: "queued", message: null, percent: null },
      });
    } else {
      publish(hash, { status: "queued", message: null, percent: null });
    }
    try {
      await invoke("transcribe", { hash, replace });
    } catch (error) {
      if (replace) {
        publish(hash, {
          replacement: {
            status: "failed",
            message: String(error),
            percent: null,
          },
        });
      } else {
        publish(hash, {
          status: "failed",
          message: String(error),
          percent: null,
        });
      }
      log.error("transcribe start failed", toErrorFields(error));
      reportActionFailure(
        "transcription-start-failed",
        "Couldn’t start transcription.",
        error,
      );
    }
  },

  cancel: async () => {
    try {
      await invoke("transcribe_cancel");
    } catch (error) {
      log.warn("transcription cancellation failed", toErrorFields(error));
      reportActionFailure(
        "transcription-cancel-failed",
        "Couldn’t cancel transcription.",
        error,
      );
    }
  },
}));

void (async () => {
  try {
    await listen<{ hash: string; percent: number }>(
      "transcribe://progress",
      (event) => {
        active = event.payload;
        const current = useTranscriptStore.getState().rows[event.payload.hash];
        if (
          current?.replacement !== null &&
          current?.replacement !== undefined
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
    await listen<{ hash: string; text: string }>(
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
    await listen<{ hash: string; message: string }>(
      "transcribe://error",
      (event) => {
        if (active?.hash === event.payload.hash) active = null;
        const current = useTranscriptStore.getState().rows[event.payload.hash];
        if (
          current?.replacement !== null &&
          current?.replacement !== undefined
        ) {
          publishIfLoaded(event.payload.hash, {
            replacement: {
              status: "failed",
              message: event.payload.message,
              percent: null,
            },
          });
        } else {
          publishIfLoaded(event.payload.hash, {
            status: "failed",
            message: event.payload.message,
            percent: null,
          });
        }
      },
    );
    await listen<{ hash: string }>("transcribe://cancelled", (event) => {
      if (active?.hash === event.payload.hash) active = null;
      const current = useTranscriptStore.getState().rows[event.payload.hash];
      if (current?.replacement !== null && current?.replacement !== undefined) {
        publishIfLoaded(event.payload.hash, { replacement: null });
      } else {
        publishIfLoaded(event.payload.hash, {
          status: "pending",
          message: null,
          percent: null,
        });
      }
    });
  } catch (error) {
    log.warn("transcript event wiring failed", toErrorFields(error));
    const message = error instanceof Error ? error.message : String(error);
    recordInterfaceFailure(message);
    const interrupted = active as { hash: string; percent: number } | null;
    if (interrupted !== null) {
      publishIfLoaded(interrupted.hash, {
        status: "failed",
        message:
          "Live transcription updates are unavailable. Restart OneCopy to repair them.",
        percent: null,
      });
      active = null;
    }
  }
})();
