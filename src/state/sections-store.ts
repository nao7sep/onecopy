// Left-pane sections + scan lifecycle. I/O stays in the repositories/commands;
// this store holds the data and the actions the shell binds to. Scan progress
// arrives as Tauri events (the core emits scan://progress / done / error from
// the worker thread) and counts refresh on completion.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { log, toErrorFields } from "../repositories";
import { progressLine } from "../models/scan";
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

export const useSectionsStore = create<SectionsState>((set) => ({
  counts: null,
  scanning: false,
  progress: "",
  rescanNeeded: false,

  loadCounts: async () => {
    try {
      const counts = await invoke<SectionCounts>("get_section_counts");
      set({ counts });
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

// Event wiring, installed once at module load. Fire-and-forget: a listen
// failure logs and leaves the store on manual refresh only.
void (async () => {
  try {
    await listen<{ phase: string; detail: string }>("scan://progress", (event) => {
      useSectionsStore.setState({
        scanning: true,
        progress: progressLine(event.payload.phase, event.payload.detail),
      });
    });
    await listen<{ cancelled?: boolean }>("scan://done", (event) => {
      // A cancelled scan is NOT a clean finish: the walk stopped partway, so
      // whole directories may still be unread and the counts below understate
      // the library. The next launch re-walks (the root stays walk-owed), but
      // saying nothing here is what made "months look emptier than I know they
      // are" impossible to attribute.
      const cancelled = event.payload?.cancelled === true;
      useSectionsStore.setState({
        scanning: false,
        progress: "",
        rescanNeeded: cancelled || useSectionsStore.getState().rescanNeeded,
      });
      void useSectionsStore.getState().loadCounts();
      // The open section may have gained or lost items; the issues count may
      // have grown.
      void import("./items-store").then(({ useItemsStore }) =>
        useItemsStore.getState().refresh(),
      );
      void import("./issues-store").then(({ useIssuesStore }) =>
        useIssuesStore.getState().load(),
      );
    });
    await listen<{ message: string }>("scan://error", (event) => {
      useSectionsStore.setState({ scanning: false, progress: "" });
      log.error("scan failed", { error: { message: event.payload.message } });
    });
    // The watcher's live updates: new/changed files flowed through the
    // pipeline in the background — refresh the views.
    await listen("watch://updated", () => {
      void useSectionsStore.getState().loadCounts();
      void import("./items-store").then(({ useItemsStore }) =>
        useItemsStore.getState().refresh(),
      );
      void import("./issues-store").then(({ useIssuesStore }) =>
        useIssuesStore.getState().load(),
      );
    });
    await listen("watch://rescan-needed", () => {
      useSectionsStore.setState({ rescanNeeded: true });
    });
  } catch (error) {
    log.warn("scan event wiring failed", toErrorFields(error));
  }
})();
