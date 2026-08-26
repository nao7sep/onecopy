// Left-pane sections + scan lifecycle. I/O stays in the repositories/commands;
// this store owns only its data and direct commands. Cross-store reactions to
// scan/watch events live at the application edge in workflows/scan-events.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";
import { requestSeq } from "./request-seq";
import type { SectionCounts } from "../models/sections";

interface SectionsState {
  counts: SectionCounts | null;
  scanning: boolean;
  progress: string;
  /** The watcher lost events (overflow); a manual Scan repairs the index. */
  rescanNeeded: boolean;
  loadCounts: () => Promise<void>;
  startScan: () => Promise<void>;
}

const countsLoad = requestSeq();

export const useSectionsStore = create<SectionsState>((set) => ({
  counts: null,
  scanning: false,
  progress: "",
  rescanNeeded: false,

  loadCounts: async () => {
    // Async command: with the per-phase reloads several of these can be in
    // flight, and an older snapshot landing last would roll the tree
    // backwards until the next refresh (request-seq.ts).
    const fresh = countsLoad.begin();
    try {
      const counts = await invoke<SectionCounts>("get_section_counts");
      if (fresh()) set({ counts });
    } catch (error) {
      log.error("section counts load failed", toErrorFields(error));
    }
  },

  startScan: async () => {
    try {
      const started = await invoke<boolean>("start_scan");
      if (started) {
        set({ scanning: true, progress: "Scanning…", rescanNeeded: false });
        log.info("scan started");
      }
    } catch (error) {
      log.error("scan start failed", toErrorFields(error));
      set({ scanning: false, progress: "" });
    }
  },
}));
