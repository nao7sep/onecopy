// Left-pane counts and the two independent library-reconciliation lifecycles.
// Event adapters update the snapshots; this store sends direct user commands.

import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import type { ScanProgress } from "../models/scan";
import type { SectionCounts } from "../models/sections";
import { log, toErrorFields } from "../repositories";
import { requestSeq } from "./request-seq";
import { recordActionFailure } from "./notifications-store";

export interface SourceCheckState {
  running: boolean;
  stopping: boolean;
  lastResult: "stopped" | "completed" | "failed";
  eventSequence: number;
  progress: ScanProgress | null;
}

export interface FileInformationState {
  running: boolean;
  paused: boolean;
  stopping: boolean;
  queued: boolean;
  eventSequence: number;
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
  startSourceCheck: () => Promise<boolean>;
  stopSourceCheck: () => Promise<void>;
  admitBackgroundCompletion: () => Promise<void>;
  setFileInformationPaused: (paused: boolean) => Promise<void>;
}

const countsLoad = requestSeq();
const workLoad = requestSeq();

const initialSourceCheck: SourceCheckState = {
  running: false,
  stopping: false,
  lastResult: "stopped",
  eventSequence: 0,
  progress: null,
};

const initialFileInformation: FileInformationState = {
  running: false,
  paused: false,
  stopping: false,
  queued: false,
  eventSequence: 0,
  progress: null,
};

export const useSectionsStore = create<SectionsState>((set, get) => ({
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
      if (fresh()) {
        set({ error: "Couldn’t read the library sections." });
        recordActionFailure(
          "section-counts-load-failed",
          "Couldn’t read the library sections.",
          error,
        );
      }
    }
  },

  loadIndexWork: async () => {
    const fresh = workLoad.begin();
    try {
      const snapshot = await invoke<IndexWorkSnapshot>("index_work_snapshot");
      if (!fresh()) return;
      set((state) => ({
        error: null,
        sourceCheck:
          snapshot.sourceCheck.eventSequence < state.sourceCheck.eventSequence
            ? state.sourceCheck
            : {
                ...snapshot.sourceCheck,
                progress: snapshot.sourceCheck.running ? state.sourceCheck.progress : null,
              },
        fileInformation:
          snapshot.fileInformation.eventSequence < state.fileInformation.eventSequence
            ? state.fileInformation
            : {
                ...snapshot.fileInformation,
                progress: snapshot.fileInformation.running
                  ? state.fileInformation.progress
                  : null,
              },
      }));
    } catch (error) {
      log.error("library background-work status failed", toErrorFields(error));
      if (fresh()) {
        set({ error: "Couldn’t read library background-work status." });
        recordActionFailure(
          "background-work-status-failed",
          "Couldn’t read library background-work status.",
          error,
        );
      }
    }
  },

  startSourceCheck: async () => {
    set({ error: null });
    try {
      const started = await invoke<boolean>("start_source_check");
      if (started) {
        set({ rescanNeeded: false });
        await get().loadIndexWork();
        log.info("source-folder check started");
      }
      return started;
    } catch (error) {
      log.error("source-folder check start failed", toErrorFields(error));
      set({ error: "Couldn’t start checking source folders." });
      recordActionFailure("source-check-start-failed", "Couldn’t start checking source folders.", error);
      return false;
    }
  },

  stopSourceCheck: async () => {
    set({ error: null });
    try {
      const accepted = await invoke<boolean>("stop_source_check");
      if (accepted) {
        await get().loadIndexWork();
      }
    } catch (error) {
      log.error("source-folder stop failed", toErrorFields(error));
      set({ error: "Couldn’t stop checking source folders." });
      recordActionFailure("source-check-stop-failed", "Couldn’t stop checking source folders.", error);
    }
  },

  admitBackgroundCompletion: async () => {
    try {
      await invoke("admit_background_completion");
    } catch (error) {
      log.error("file-information startup failed", toErrorFields(error));
      set({ error: "Couldn’t start completing file information." });
      recordActionFailure(
        "file-information-start-failed",
        "Couldn’t start completing file information.",
        error,
      );
    }
  },

  setFileInformationPaused: async (paused) => {
    set({ error: null });
    try {
      await invoke("set_file_information_paused", { paused });
      await get().loadIndexWork();
    } catch (error) {
      log.error("file-information pause change failed", toErrorFields(error));
      set({ error: "Couldn’t change file-information background work." });
      recordActionFailure(
        "file-information-control-failed",
        "Couldn’t change file-information background work.",
        error,
      );
    }
  },
}));
