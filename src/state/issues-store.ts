// The current-run inbox is diagnostic visibility, never work eligibility.
import { create } from "zustand";
import { requestSeq } from "./request-seq";
import { invoke } from "@tauri-apps/api/core";
import { message, type Message } from "../i18n/translate";
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
}

interface IssuesState {
  total: number;
  rows: IssueRow[];
  loading: boolean;
  error: Message | null;
  load: () => Promise<void>;
  dismiss: (id: number) => Promise<void>;
  dismissAll: () => Promise<void>;
}

const issuesLoad = requestSeq();
export const useIssuesStore = create<IssuesState>((set, get) => ({
  total: 0,
  rows: [],
  loading: false,
  error: null,
  load: async () => {
    const fresh = issuesLoad.begin();
    set({ loading: true, error: null });
    try {
      const result = await invoke<{ total: number; rows: IssueRow[] }>("get_issues", { limit: 500 });
      if (fresh()) set({ ...result, loading: false, error: null });
    } catch (error) {
      if (!fresh()) return;
      log.error("issues load failed", toErrorFields(error));
      set({ loading: false, error: message("issues.unavailable") });
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
