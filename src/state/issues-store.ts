// Issues are CURRENT-STATE diagnostics, not a log — everything the pipeline
// could not do lands here, one row per (kind, path). Scan-derived rows clear
// themselves when a scan finds the condition resolved; safe derived failures
// may be retried, while operation records wait for the user's Dismiss. The
// count lives in the status bar (nothing at zero),
// and there are deliberately NO toasts anywhere: the design case is a
// multi-day unattended scan, where anything transient would be missed.

import { create } from "zustand";
import { requestSeq } from "./request-seq";
import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";

export interface IssueRow {
  id: number;
  path: string | null;
  kind: string;
  message: string | null;
  firstSeenUtc: string;
  lastSeenUtc: string;
  recovery: {
    action: "retry" | "recheck";
    label: string;
    status: "available" | "queued" | "running";
  } | null;
}

interface RecheckResult {
  status: "started" | "busy" | "stillFailing" | "notRecoverable";
}

interface IssuesState {
  total: number;
  rows: IssueRow[];
  loading: boolean;
  error: string | null;
  /** The Issues modal (a plain modal, not persisted — a diagnostics window
   * is something you open, read, and close). */
  open: boolean;
  load: () => Promise<void>;
  setOpen: (open: boolean) => void;
  dismiss: (id: number) => Promise<void>;
  dismissAll: () => Promise<void>;
  recover: (id: number) => Promise<void>;
  retryAll: () => Promise<void>;
}

const issuesLoad = requestSeq();

export const useIssuesStore = create<IssuesState>((set, get) => ({
  total: 0,
  rows: [],
  loading: false,
  error: null,
  open: false,

  load: async () => {
    // Async command — the older of two in-flight loads must lose (request-seq.ts).
    const fresh = issuesLoad.begin();
    set({ loading: true, error: null });
    try {
      const result = await invoke<{ total: number; rows: IssueRow[] }>("get_issues", {
        limit: 500,
      });
      if (fresh()) set({ total: result.total, rows: result.rows, loading: false, error: null });
    } catch (error) {
      log.error("issues load failed", toErrorFields(error));
      if (fresh()) set({ loading: false, error: "Issues are unavailable." });
    }
  },

  setOpen: (open) => set({ open }),

  dismiss: async (id) => {
    set({ error: null });
    try {
      await invoke("dismiss_issue", { id });
      await get().load();
    } catch (error) {
      log.error("issue dismissal failed", toErrorFields(error));
      set({ error: "Couldn’t dismiss the issue." });
    }
  },

  dismissAll: async () => {
    set({ error: null });
    try {
      await invoke("dismiss_all_issues");
      await get().load();
    } catch (error) {
      log.error("dismiss all failed", toErrorFields(error));
      set({ error: "Couldn’t dismiss the issues." });
    }
  },

  recover: async (id) => {
    const recovery = get().rows.find((row) => row.id === id)?.recovery;
    if (!recovery || recovery.status !== "available") return;
    set({ error: null });
    set((state) => ({
      rows: state.rows.map((row) =>
        row.id === id && row.recovery
          ? { ...row, recovery: { ...row.recovery, status: "running" } }
          : row,
      ),
    }));
    try {
      if (recovery.action === "recheck") {
        const result = await invoke<RecheckResult>("recheck_issue", { id });
        await get().load();
        if (result.status === "busy") {
          set({ error: "Indexing is busy. Recheck when it finishes." });
        } else if (result.status === "stillFailing") {
          set({ error: "The filesystem condition is still present." });
        } else if (result.status === "notRecoverable") {
          set({ error: "Recovery is no longer available." });
        }
      } else {
        await invoke("retry_issue", { id });
        await get().load();
      }
    } catch (error) {
      log.error("issue recovery failed", toErrorFields(error));
      await get().load();
      set({ error: "Couldn’t run the recovery." });
    }
  },

  retryAll: async () => {
    set({ error: null });
    try {
      await invoke("retry_all_issues");
      await get().load();
    } catch (error) {
      log.error("retry all issues failed", toErrorFields(error));
      set({ error: "Couldn’t retry the issues." });
    }
  },
}));
