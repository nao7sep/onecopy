// Main-window projection of the one ephemeral backend mutation runtime.
// Plans, results, retryability, and durable failures stay with the owning
// operation; this store carries only what the status bar needs right now.

import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import type { MutationProgress } from "../models/mutation";
import { log, toErrorFields } from "../repositories";

interface MutationState {
  progress: MutationProgress | null;
  cancelling: boolean;
  cancel: () => Promise<void>;
}

export const useMutationStore = create<MutationState>((set, get) => ({
  progress: null,
  cancelling: false,

  cancel: async () => {
    const progress = get().progress;
    if (progress === null || get().cancelling) return;
    set({ cancelling: true });
    try {
      const accepted = await invoke<boolean>("mutation_cancel", {
        operationId: progress.operationId,
      });
      if (!accepted && get().progress?.operationId === progress.operationId) {
        set({ cancelling: false });
      }
    } catch (error) {
      log.error("file operation cancellation failed", toErrorFields(error));
      if (get().progress?.operationId === progress.operationId) {
        set({ cancelling: false });
      }
    }
  },
}));
