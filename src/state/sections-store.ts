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
  loadCounts: () => Promise<void>;
  startScan: () => Promise<void>;
}

export const useSectionsStore = create<SectionsState>((set) => ({
  counts: null,
  scanning: false,
  progress: "",

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
        set({ scanning: true, progress: "Scanning…" });
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
    });
    await listen<{ message: string }>("scan://error", (event) => {
      useSectionsStore.setState({ scanning: false, progress: "" });
      log.error("scan failed", { error: { message: event.payload.message } });
    });
  } catch (error) {
    log.warn("scan event wiring failed", toErrorFields(error));
  }
})();
