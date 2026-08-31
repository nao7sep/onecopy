// Left-pane counts and the two independent library-reconciliation lifecycles.
// Event adapters update the snapshots; this store sends direct user commands.

import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import type { ScanProgress } from "../models/scan";
import type { SectionCounts } from "../models/sections";
import { log, toErrorFields } from "../repositories";
import { requestSeq } from "./request-seq";
import { reportActionFailure } from "./notifications-store";

export interface SourceCheckState {
  running: boolean;
  stopping: boolean;
  progress: ScanProgress | null;
}

export interface FileInformationState {
  running: boolean;
  paused: boolean;
  stopping: boolean;
  queued: boolean;
  progress: ScanProgress | null;
}

interface IndexWorkSnapshot {
  sourceCheck: Omit<SourceCheckState, "progress">;
  fileInformation: Omit<FileInformationState, "progress">;
}

interface SectionsState {
  counts: SectionCounts | null;
  error: string | null;
  sourceCheck: SourceCheckState;
  fileInformation: FileInformationState;
  /** Watcher overflow or a stopped source walk requires explicit discovery. */
  rescanNeeded: boolean;
  loadCounts: () => Promise<void>;
  loadIndexWork: () => Promise<void>;
  startSourceCheck: () => Promise<void>;
  stopSourceCheck: () => Promise<void>;
  setFileInformationPaused: (paused: boolean) => Promise<void>;
}

const countsLoad = requestSeq();
const workLoad = requestSeq();

const initialSourceCheck: SourceCheckState = {
  running: false,
  stopping: false,
  progress: null,
};

const initialFileInformation: FileInformationState = {
  running: false,
  paused: false,
  stopping: false,
  queued: false,
  progress: null,
};

export const useSectionsStore = create<SectionsState>((set) => ({
  counts: null,
  error: null,
  sourceCheck: initialSourceCheck,
  fileInformation: initialFileInformation,
  rescanNeeded: false,

  loadCounts: async () => {
    const fresh = countsLoad.begin();
    try {
      const counts = await invoke<SectionCounts>("get_section_counts");
      if (fresh()) set({ counts, error: null });
    } catch (error) {
      log.error("section counts load failed", toErrorFields(error));
      if (fresh()) set({ error: "Couldn’t read the library sections." });
    }
  },

  loadIndexWork: async () => {
    const fresh = workLoad.begin();
    try {
      const snapshot = await invoke<IndexWorkSnapshot>("index_work_snapshot");
      if (!fresh()) return;
      set((state) => ({
        error: null,
        sourceCheck: { ...snapshot.sourceCheck, progress: state.sourceCheck.progress },
        fileInformation: {
          ...snapshot.fileInformation,
          progress: state.fileInformation.progress,
        },
      }));
    } catch (error) {
      log.error("library background-work status failed", toErrorFields(error));
      if (fresh()) set({ error: "Couldn’t read library background-work status." });
    }
  },

  startSourceCheck: async () => {
    set({ error: null });
    try {
      const started = await invoke<boolean>("start_source_check");
      if (started) {
        set({
          sourceCheck: { running: true, stopping: false, progress: null },
          rescanNeeded: false,
        });
        log.info("source-folder check started");
      }
    } catch (error) {
      log.error("source-folder check start failed", toErrorFields(error));
      set({ error: "Couldn’t start checking source folders." });
      reportActionFailure("source-check-start-failed", "Couldn’t start checking source folders.", error);
    }
  },

  stopSourceCheck: async () => {
    set({ error: null });
    try {
      const accepted = await invoke<boolean>("stop_source_check");
      if (accepted) {
        set((state) => ({
          sourceCheck: { ...state.sourceCheck, stopping: true },
        }));
      }
    } catch (error) {
      log.error("source-folder stop failed", toErrorFields(error));
      set({ error: "Couldn’t stop checking source folders." });
      reportActionFailure("source-check-stop-failed", "Couldn’t stop checking source folders.", error);
    }
  },

  setFileInformationPaused: async (paused) => {
    set({ error: null });
    try {
      await invoke("set_file_information_paused", { paused });
      set((state) => ({
        fileInformation: {
          ...state.fileInformation,
          paused,
          stopping: paused && state.fileInformation.running,
        },
      }));
    } catch (error) {
      log.error("file-information pause change failed", toErrorFields(error));
      set({ error: "Couldn’t change file-information background work." });
      reportActionFailure(
        "file-information-control-failed",
        "Couldn’t change file-information background work.",
        error,
      );
    }
  },
}));
