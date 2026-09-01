// Active owns unresolved current conditions; Recent owns restart-persistent
// notification history. They share one modal but retain different lifecycles:
// resolving/dismissing Active never erases Recent, and dismissing a live notice
// never edits its history row.

import { create } from "zustand";
import { requestSeq } from "./request-seq";
import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";
import { recordActionFailure } from "./notifications-store";

export interface IssueRow {
  id: number;
  path: string | null;
  kind: string;
  message: string | null;
  firstSeenUtc: string;
  lastSeenUtc: string;
  occurrenceCount: number;
  recovery: {
    action: "retry" | "recheck";
    label: string;
    status: "available" | "queued" | "running";
  } | null;
}

export interface RecentNotificationRow {
  id: number;
  kind: string;
  path: string | null;
  level: "info" | "warning" | "error";
  presentation: "timed" | "persistent";
  message: string;
  firstSeenUtc: string;
  lastSeenUtc: string;
  occurrenceCount: number;
}

interface RecheckResult {
  status: "started" | "busy" | "stillFailing" | "notRecoverable";
}

interface IssuesState {
  total: number;
  rows: IssueRow[];
  loading: boolean;
  error: string | null;
  recentTotal: number;
  recentRows: RecentNotificationRow[];
  recentLoading: boolean;
  recentError: string | null;
  view: "active" | "recent";
  /** The Issues modal (a plain modal, not persisted — a diagnostics window
   * is something you open, read, and close). */
  open: boolean;
  load: () => Promise<void>;
  loadActive: () => Promise<void>;
  loadRecent: () => Promise<void>;
  setOpen: (open: boolean) => void;
  setView: (view: "active" | "recent") => void;
  dismiss: (id: number) => Promise<void>;
  dismissAll: () => Promise<void>;
  recover: (id: number) => Promise<void>;
  retryAll: () => Promise<void>;
}

const issuesLoad = requestSeq();
const recentLoad = requestSeq();

export const useIssuesStore = create<IssuesState>((set, get) => ({
  total: 0,
  rows: [],
  loading: false,
  error: null,
  recentTotal: 0,
  recentRows: [],
  recentLoading: false,
  recentError: null,
  view: "active",
  open: false,

  load: async () => {
    await Promise.all([get().loadActive(), get().loadRecent()]);
  },

  loadActive: async () => {
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

  loadRecent: async () => {
    const fresh = recentLoad.begin();
    set({ recentLoading: true, recentError: null });
    try {
      const result = await invoke<{ total: number; rows: RecentNotificationRow[] }>(
        "get_recent_notifications",
        { limit: 500 },
      );
      if (fresh()) {
        set({
          recentTotal: result.total,
          recentRows: result.rows,
          recentLoading: false,
          recentError: null,
        });
      }
    } catch (error) {
      log.error("recent notifications load failed", toErrorFields(error));
      if (fresh()) {
        set({ recentLoading: false, recentError: "Recent notifications are unavailable." });
      }
    }
  },

  setOpen: (open) => set({ open }),
  setView: (view) => set({ view }),

  dismiss: async (id) => {
    set({ error: null });
    try {
      await invoke("dismiss_issue", { id });
      await get().loadActive();
    } catch (error) {
      log.error("issue dismissal failed", toErrorFields(error));
      set({ error: "Couldn’t dismiss the issue." });
      recordActionFailure("issue-dismiss-failed", "Couldn’t dismiss the issue.", error);
    }
  },

  dismissAll: async () => {
    set({ error: null });
    try {
      await invoke("dismiss_all_issues");
      await get().loadActive();
    } catch (error) {
      log.error("dismiss all failed", toErrorFields(error));
      set({ error: "Couldn’t dismiss the issues." });
      recordActionFailure("issues-dismiss-failed", "Couldn’t dismiss the issues.", error);
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
        await get().loadActive();
        if (result.status === "busy") {
          set({ error: "Indexing is busy. Recheck when it finishes." });
        } else if (result.status === "stillFailing") {
          set({ error: "The filesystem condition is still present." });
        } else if (result.status === "notRecoverable") {
          set({ error: "Recovery is no longer available." });
        }
      } else {
        await invoke("retry_issue", { id });
        await get().loadActive();
      }
    } catch (error) {
      log.error("issue recovery failed", toErrorFields(error));
      await get().loadActive();
      set({ error: "Couldn’t run the recovery." });
      recordActionFailure("issue-recovery-failed", "Couldn’t run the issue recovery.", error);
    }
  },

  retryAll: async () => {
    set({ error: null });
    try {
      await invoke("retry_all_issues");
      await get().loadActive();
    } catch (error) {
      log.error("retry all issues failed", toErrorFields(error));
      set({ error: "Couldn’t retry the issues." });
      recordActionFailure("issues-retry-failed", "Couldn’t retry the issues.", error);
    }
  },
}));
