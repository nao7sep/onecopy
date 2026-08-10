// First-run wizard state + the volume-presence gate. The wizard opens when no
// source directories are configured (the inbox-zero handler has nothing to
// handle without them); the gate blocks work mode when configured directories
// are absent (an unmounted volume manifests as a missing directory).

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { log, toErrorFields } from "../repositories";

export interface QuickCount {
  images: number;
  videos: number;
  others: number;
}

export interface WizardDir {
  path: string;
  counts: QuickCount | null;
  counting: boolean;
}

interface WizardState {
  open: boolean;
  step: 1 | 2 | 3;
  dirs: WizardDir[];
  timezone: string;
  timezoneValid: boolean;
  cacheDir: string | null;
  missingDirs: string[];
  init: (config: Record<string, unknown> | null, dataRoot: string) => Promise<void>;
  addDirs: () => Promise<void>;
  removeDir: (path: string) => void;
  setStep: (step: 1 | 2 | 3) => void;
  setTimezone: (name: string) => Promise<void>;
  pickCacheDir: () => Promise<void>;
  finish: () => Promise<void>;
  recheckPresence: () => Promise<void>;
}

export const useWizardStore = create<WizardState>((set, get) => ({
  open: false,
  step: 1,
  dirs: [],
  timezone: "",
  timezoneValid: true,
  cacheDir: null,
  missingDirs: [],

  init: async (config, _dataRoot) => {
    const sourceDirs = Array.isArray(config?.sourceDirs)
      ? (config.sourceDirs as string[])
      : [];
    const timezone =
      typeof config?.defaultTimezone === "string" ? config.defaultTimezone : "UTC";
    const cacheDir =
      typeof config?.cacheDir === "string" && config.cacheDir.trim() !== ""
        ? config.cacheDir
        : null;
    if (sourceDirs.length === 0) {
      set({ open: true, step: 1, dirs: [], timezone, cacheDir });
    } else {
      set({ open: false, timezone, cacheDir });
      await get().recheckPresence();
    }
  },

  addDirs: async () => {
    try {
      const picked = await openDialog({ directory: true, multiple: true });
      const paths = (Array.isArray(picked) ? picked : picked ? [picked] : []).filter(
        (p): p is string => typeof p === "string",
      );
      const existing = new Set(get().dirs.map((d) => d.path));
      const fresh = paths.filter((p) => !existing.has(p));
      if (fresh.length === 0) return;
      set({
        dirs: [
          ...get().dirs,
          ...fresh.map((path) => ({ path, counts: null, counting: true })),
        ],
      });
      for (const path of fresh) {
        void invoke<QuickCount>("quick_count", { root: path })
          .then((counts) => {
            // A directory removed while counting stays removed (stale guard).
            set({
              dirs: get().dirs.map((d) =>
                d.path === path ? { ...d, counts, counting: false } : d,
              ),
            });
          })
          .catch((error) => {
            log.error("quick count failed", toErrorFields(error));
            set({
              dirs: get().dirs.map((d) =>
                d.path === path ? { ...d, counting: false } : d,
              ),
            });
          });
      }
    } catch (error) {
      log.error("directory picker failed", toErrorFields(error));
    }
  },

  removeDir: (path) => {
    // Stop an in-flight count's disk churn, not just its result.
    void invoke("cancel_quick_count", { root: path }).catch(() => {});
    set({ dirs: get().dirs.filter((d) => d.path !== path) });
  },

  setStep: (step) => set({ step }),

  setTimezone: async (name) => {
    set({ timezone: name });
    try {
      const valid = await invoke<boolean>("validate_timezone", { name });
      if (get().timezone === name) set({ timezoneValid: valid });
    } catch {
      set({ timezoneValid: false });
    }
  },

  pickCacheDir: async () => {
    try {
      const picked = await openDialog({ directory: true, multiple: false });
      if (typeof picked === "string") set({ cacheDir: picked });
    } catch (error) {
      log.error("cache dir picker failed", toErrorFields(error));
    }
  },

  finish: async () => {
    const { dirs, timezone, cacheDir } = get();
    try {
      // A patch of exactly the wizard's three keys through the one config
      // owner; everything else in config.json stays untouched.
      const { useAppStore } = await import("./app-store");
      await useAppStore.getState().patchConfig({
        sourceDirs: dirs.map((d) => d.path),
        defaultTimezone: timezone,
        cacheDir,
      });
      set({ open: false });
      log.info("wizard finished", { sourceDirs: dirs.length });
      const { useSectionsStore } = await import("./sections-store");
      await useSectionsStore.getState().startScan();
    } catch (error) {
      log.error("wizard save failed", toErrorFields(error));
    }
  },

  recheckPresence: async () => {
    try {
      const missing = await invoke<string[]>("check_source_dirs");
      set({ missingDirs: missing });
    } catch (error) {
      log.error("presence check failed", toErrorFields(error));
    }
  },
}));
