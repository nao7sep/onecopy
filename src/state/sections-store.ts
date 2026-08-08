// Left-pane sections + scan lifecycle. I/O stays in the repositories/commands;
// this store holds the data and the actions the shell binds to. Scan progress
// arrives as Tauri events (the core emits scan://progress / done / error from
// the worker thread) and counts refresh on completion.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { log, toErrorFields } from "../repositories";
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
        progress: `${event.payload.phase}: ${event.payload.detail}`,
      });
    });
    await listen("scan://done", () => {
      useSectionsStore.setState({ scanning: false, progress: "" });
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
