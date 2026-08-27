// Left-pane sections + scan lifecycle. I/O stays in the repositories/commands;
// this store owns only its data and direct commands. Cross-store reactions to
// scan/watch events live at the application edge in workflows/scan-events.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";
import { requestSeq } from "./request-seq";
import type { SectionCounts } from "../models/sections";
import type { ScanProgress } from "../models/scan";

interface SectionsState {
  counts: SectionCounts | null;
  scanning: boolean;
  stopping: boolean;
  progress: ScanProgress | null;
  /** The watcher lost events (overflow); a manual Scan repairs the index. */
  rescanNeeded: boolean;
  loadCounts: () => Promise<void>;
  startScan: () => Promise<void>;
  cancelScan: () => Promise<void>;
}

const countsLoad = requestSeq();

export const useSectionsStore = create<SectionsState>((set) => ({
  counts: null,
  scanning: false,
  stopping: false,
  progress: null,
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
        set({ scanning: true, stopping: false, progress: null, rescanNeeded: false });
        log.info("scan started");
      }
    } catch (error) {
      log.error("scan start failed", toErrorFields(error));
      set({ scanning: false, stopping: false, progress: null });
    }
  },

  cancelScan: async () => {
    try {
      const accepted = await invoke<boolean>("cancel_scan");
      if (accepted) set({ stopping: true });
    } catch (error) {
      log.error("scan cancellation failed", toErrorFields(error));
    }
  },
}));
