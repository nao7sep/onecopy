// Issues are CURRENT-STATE diagnostics, not a log — everything the pipeline
// could not do lands here, one row per (kind, path). Scan-derived rows clear
// themselves when a scan finds the condition resolved; operation records wait
// for the user's Dismiss. The count lives in the status bar (nothing at zero),
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
}

interface IssuesState {
  total: number;
  rows: IssueRow[];
  /** The Issues modal (a plain modal, not persisted — a diagnostics window
   * is something you open, read, and close). */
  open: boolean;
  load: () => Promise<void>;
  setOpen: (open: boolean) => void;
  dismiss: (id: number) => Promise<void>;
  dismissAll: () => Promise<void>;
}

const issuesLoad = requestSeq();

export const useIssuesStore = create<IssuesState>((set, get) => ({
  total: 0,
  rows: [],
  open: false,

  load: async () => {
    // Async command — the older of two in-flight loads must lose (request-seq.ts).
    const fresh = issuesLoad.begin();
    try {
      const result = await invoke<{ total: number; rows: IssueRow[] }>("get_issues", {
        limit: 500,
      });
      if (fresh()) set({ total: result.total, rows: result.rows });
    } catch (error) {
      log.error("issues load failed", toErrorFields(error));
    }
  },

  setOpen: (open) => set({ open }),

  dismiss: async (id) => {
    try {
      await invoke("dismiss_issue", { id });
      await get().load();
    } catch (error) {
      log.error("issue dismissal failed", toErrorFields(error));
    }
  },

  dismissAll: async () => {
    try {
      await invoke("dismiss_all_issues");
      await get().load();
    } catch (error) {
      log.error("dismiss all failed", toErrorFields(error));
    }
  },
}));
