// The first-class issues surface: everything the pipeline could not do lands
// here — "the app never mentioned it" must never mean "the app never saw it".

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";

export interface IssueRow {
  id: number;
  path: string | null;
  kind: string;
  message: string | null;
  createdAtUtc: string;
}

interface IssuesState {
  total: number;
  rows: IssueRow[];
  open: boolean;
  load: () => Promise<void>;
  setOpen: (open: boolean) => void;
}

export const useIssuesStore = create<IssuesState>((set) => ({
  total: 0,
  rows: [],
  open: false,

  load: async () => {
    try {
      const result = await invoke<{ total: number; rows: IssueRow[] }>("get_issues", {
        limit: 500,
      });
      set({ total: result.total, rows: result.rows });
    } catch (error) {
      log.error("issues load failed", toErrorFields(error));
    }
  },

  setOpen: (open) => {
    set({ open });
    void import("./app-store").then(({ useAppStore }) =>
      useAppStore.getState().patchState({ issuesOpen: open }),
    );
  },
}));
