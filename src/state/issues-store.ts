// The current-run inbox is diagnostic visibility, never work eligibility.
import { create } from "zustand";
import { requestSeq } from "./request-seq";
import { invoke } from "@tauri-apps/api/core";
import { message, type Message } from "../i18n/translate";
import type { MessageKey } from "../i18n/catalogues";
import { log, toErrorFields } from "../repositories";
import { recordActionFailure, type StoredMessageValues } from "./notifications-store";

export interface IssueRow {
  id: number;
  path: string | null;
  kind: string;
  message: string | null;
  /** A catalogue key the frontend renders in the current interface language;
   * absent for a row recorded before this descriptor existed (R5.5 D-L12). */
  messageKey: MessageKey | null;
  messageValues: StoredMessageValues | null;
  firstSeenUtc: string;
  lastSeenUtc: string;
  occurrenceCount: number;
}

const PAGE_SIZE = 500;

interface IssuesState {
  total: number;
  rows: IssueRow[];
  loading: boolean;
  /** A later page loading on top of what's already shown, distinct from the
   * first load's own spinner (R4.4 finding A: a row past the first page must
   * stay reachable, not hidden behind dismissing everything ahead of it). */
  loadingMore: boolean;
  error: Message | null;
  load: () => Promise<void>;
  loadMore: () => Promise<void>;
  dismiss: (id: number) => Promise<void>;
  dismissAll: () => Promise<void>;
}

const issuesLoad = requestSeq();
export const useIssuesStore = create<IssuesState>((set, get) => ({
  total: 0,
  rows: [],
  loading: false,
  loadingMore: false,
  error: null,
  load: async () => {
    const fresh = issuesLoad.begin();
    set({ loading: true, loadingMore: false, error: null });
    try {
      const result = await invoke<{ total: number; rows: IssueRow[] }>("get_issues", { limit: PAGE_SIZE });
      if (fresh()) set({ ...result, loading: false, error: null });
    } catch (error) {
      if (!fresh()) return;
      log.error("issues load failed", toErrorFields(error));
      set({ loading: false, error: message("issues.unavailable") });
    }
  },
  loadMore: async () => {
    const { rows, total, loading, loadingMore } = get();
    const last = rows[rows.length - 1];
    if (loading || loadingMore || last === undefined || rows.length >= total) return;
    const fresh = issuesLoad.begin();
    set({ loadingMore: true, error: null });
    try {
      const result = await invoke<{ total: number; rows: IssueRow[] }>("get_issues", {
        limit: PAGE_SIZE,
        afterFirstSeenUtc: last.firstSeenUtc,
        afterId: last.id,
      });
      if (fresh()) {
        set((state) => ({
          total: result.total,
          rows: [...state.rows, ...result.rows],
          loadingMore: false,
          error: null,
        }));
      }
    } catch (error) {
      if (!fresh()) return;
      log.error("issues page load failed", toErrorFields(error));
      set({ loadingMore: false, error: message("issues.unavailable") });
    }
  },
  dismiss: async (id) => {
    set({ error: null });
    try {
      await invoke("dismiss_issue", { id });
      await get().load();
    } catch (error) {
      log.error("issue dismissal failed", toErrorFields(error));
      const failure = message("issues.dismissFailed");
      set({ error: failure });
      recordActionFailure("issue-dismiss-failed", failure, error);
    }
  },
  dismissAll: async () => {
    set({ error: null });
    try {
      await invoke("dismiss_all_issues");
      await get().load();
    } catch (error) {
      log.error("dismiss all failed", toErrorFields(error));
      const failure = message("issues.dismissAllFailed");
      set({ error: failure });
      recordActionFailure("issues-dismiss-failed", failure, error);
    }
  },
}));
