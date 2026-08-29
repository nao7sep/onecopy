// Left-pane counts and the two independent library-reconciliation lifecycles.
// Event adapters update the snapshots; this store sends direct user commands.

import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import type { ScanProgress } from "../models/scan";
import type { SectionCounts } from "../models/sections";
import { log, toErrorFields } from "../repositories";
import { requestSeq } from "./request-seq";

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
  sourceCheck: initialSourceCheck,
  fileInformation: initialFileInformation,
  rescanNeeded: false,

  loadCounts: async () => {
    const fresh = countsLoad.begin();
    try {
      const counts = await invoke<SectionCounts>("get_section_counts");
      if (fresh()) set({ counts });
    } catch (error) {
      log.error("section counts load failed", toErrorFields(error));
    }
  },

  loadIndexWork: async () => {
    const fresh = workLoad.begin();
    try {
      const snapshot = await invoke<IndexWorkSnapshot>("index_work_snapshot");
      if (!fresh()) return;
      set((state) => ({
        sourceCheck: { ...snapshot.sourceCheck, progress: state.sourceCheck.progress },
        fileInformation: {
          ...snapshot.fileInformation,
          progress: state.fileInformation.progress,
        },
      }));
    } catch (error) {
      log.error("library background-work status failed", toErrorFields(error));
    }
  },

  startSourceCheck: async () => {
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
    }
  },

  stopSourceCheck: async () => {
    try {
      const accepted = await invoke<boolean>("stop_source_check");
      if (accepted) {
        set((state) => ({
          sourceCheck: { ...state.sourceCheck, stopping: true },
        }));
      }
    } catch (error) {
      log.error("source-folder stop failed", toErrorFields(error));
    }
  },

  setFileInformationPaused: async (paused) => {
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
    }
  },
}));
