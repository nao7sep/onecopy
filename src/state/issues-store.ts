// The current-run inbox is diagnostic visibility, never work eligibility.
import { create } from "zustand";
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
   * absent for a row recorded before this descriptor existed. */
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
   * first load's own spinner: a row past the first page must
   * stay reachable, not hidden behind dismissing everything ahead of it. */
  loadingMore: boolean;
  error: Message | null;
  load: () => Promise<void>;
  loadMore: () => Promise<void>;
  dismiss: (id: number) => Promise<void>;
  dismissAll: () => Promise<void>;
}

export const useIssuesStore = create<IssuesState>((set, get) => {
  let reader: Promise<void> | null = null;
  let active: "refresh" | "more" | null = null;
  let refreshPending = false;
  let morePending = false;
  let dismissalRevision = 0;

  const readPending = (): Promise<void> => {
    if (reader !== null) return reader;
    // Publish ownership before callbacks can request another read.
    reader = Promise.resolve().then(async () => {
      while (refreshPending || morePending) {
        const refresh = refreshPending;
        if (refresh) refreshPending = false;
        else morePending = false;
        const { rows, total } = get();
        const last = rows[rows.length - 1];
        if (!refresh && (last === undefined || rows.length >= total)) continue;
        active = refresh ? "refresh" : "more";
        const revision = dismissalRevision;
        set({ loading: refresh, loadingMore: !refresh, error: null });
        try {
          const result = await invoke<{ total: number; rows: IssueRow[] }>("get_issues", {
            limit: PAGE_SIZE,
            ...(!refresh && last !== undefined ? {
              afterFirstSeenUtc: last.firstSeenUtc, afterId: last.id,
            } : {}),
          });
          if (revision !== dismissalRevision || (!refresh && refreshPending)) continue;
          // A notification received during this read requests one successor;
          // it does not hide the latest complete page during a sustained burst.
          set((state) => ({ ...result, rows: refresh ? result.rows : [...state.rows, ...result.rows],
            loading: false, loadingMore: false, error: null }));
        } catch (error) {
          if (revision !== dismissalRevision || (!refresh && refreshPending)) continue;
          log.error(refresh ? "issues load failed" : "issues page load failed", toErrorFields(error));
          if (refresh) morePending = false;
          set({ loading: false, loadingMore: false, error: message("issues.unavailable") });
        } finally {
          active = null;
        }
      }
      set({ loading: false, loadingMore: false });
    }).finally(() => {
      reader = null;
      // Store subscribers may request work while the final state is published.
      if (refreshPending || morePending) return readPending();
    });
    return reader;
  };

  return {
    total: 0,
    rows: [],
    loading: false,
    loadingMore: false,
    error: null,
    load: () => {
      refreshPending = true;
      // A fresh head supersedes the old page cursor, not the user's Show more intent.
      if (active === "more") morePending = true;
      return readPending();
    },
    loadMore: () => {
      if (active !== "more") morePending = true;
      return readPending();
    },
    dismiss: async (id) => {
      set({ error: null });
      try {
        await invoke("dismiss_issue", { id });
        dismissalRevision += 1;
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
        dismissalRevision += 1;
        await get().load();
      } catch (error) {
        log.error("dismiss all failed", toErrorFields(error));
        const failure = message("issues.dismissAllFailed");
        set({ error: failure });
        recordActionFailure("issues-dismiss-failed", failure, error);
      }
    },
  };
});
