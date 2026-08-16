// First-run wizard state + the volume-presence gate. The wizard opens when no
// source directories are configured (the inbox-zero handler has nothing to
// handle without them); the gate blocks work mode when configured directories
// are absent (an unmounted volume manifests as a missing directory).

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { log, toErrorFields } from "../repositories";

export interface WizardDir {
  path: string;
}

interface WizardState {
  open: boolean;
  step: 1 | 2 | 3;
  dirs: WizardDir[];
  timezone: string;
  timezoneValid: boolean;
  cacheDir: string | null;
  /** True when the wizard was RE-RUN over an existing setup. A first run has
   * nothing to return to, so only a re-run offers Cancel. */
  reconfigure: boolean;
  missingDirs: string[];
  substitutedDirs: string[];
  init: (config: Record<string, unknown> | null, dataRoot: string) => Promise<void>;
  /** Re-runs the wizard as RECONFIGURE: seeded from the current config, never
   * from empty — the only trigger a first-run wizard has after first run. */
  reopen: (config: Record<string, unknown> | null) => void;
  addDirs: () => Promise<void>;
  removeDir: (path: string) => void;
  setStep: (step: 1 | 2 | 3) => void;
  setTimezone: (name: string) => Promise<void>;
  pickCacheDir: () => Promise<void>;
  finish: () => Promise<void>;
  /** Abandons a re-run, changing nothing. Never available on a first run. */
  cancel: () => void;
  recheckPresence: () => Promise<void>;
}

export const useWizardStore = create<WizardState>((set, get) => ({
  open: false,
  step: 1,
  dirs: [],
  timezone: "",
  timezoneValid: true,
  cacheDir: null,
  reconfigure: false,
  missingDirs: [],
  substitutedDirs: [],

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
      set({ open: true, step: 1, dirs: [], timezone, cacheDir, reconfigure: false });
    } else {
      set({ open: false, timezone, cacheDir, reconfigure: false });
      await get().recheckPresence();
    }
  },

  reopen: (config) => {
    const sourceDirs = Array.isArray(config?.sourceDirs)
      ? (config.sourceDirs as string[])
      : [];
    const timezone =
      typeof config?.defaultTimezone === "string" ? config.defaultTimezone : "UTC";
    const cacheDir =
      typeof config?.cacheDir === "string" && config.cacheDir.trim() !== ""
        ? config.cacheDir
        : null;
    set({
      open: true,
      step: 1,
      timezone,
      cacheDir,
      reconfigure: true,
      dirs: sourceDirs.map((path) => ({ path })),
    });
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
      set({ dirs: [...get().dirs, ...fresh.map((path) => ({ path }))] });
    } catch (error) {
      log.error("directory picker failed", toErrorFields(error));
    }
  },

  removeDir: (path) => {
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
      // The cache root is a live process-wide value, not just a config key:
      // `patch_config` never touches it, so writing cacheDir straight into
      // config leaves derives going to the NEW directory while every
      // mediacache:// request still reads the OLD one — the whole grid stays
      // on placeholders until the next launch. `move_cache` is the only call
      // that commits both, so it owns the key here exactly as it does in
      // Settings, and cacheDir is kept out of the patch below.
      if (cacheDir !== null) {
        await invoke("move_cache", { newDir: cacheDir });
      }
      // A patch of exactly the wizard's remaining keys through the one config
      // owner; everything else in config.json stays untouched.
      const { useAppStore } = await import("./app-store");
      await useAppStore.getState().patchConfig({
        sourceDirs: dirs.map((d) => d.path),
        defaultTimezone: timezone,
      });
      set({ open: false });
      log.info("wizard finished", { sourceDirs: dirs.length });
      const { useSectionsStore } = await import("./sections-store");
      await useSectionsStore.getState().startScan();
    } catch (error) {
      log.error("wizard save failed", toErrorFields(error));
    }
  },

  cancel: () => {
    // Nothing was written on the way through — every step edits store state
    // only, and `finish` is the sole writer — so abandoning is just a close.
    set({ open: false, reconfigure: false });
  },

  recheckPresence: async () => {
    try {
      const status = await invoke<{ missing: string[]; substituted: string[] }>(
        "check_source_dirs",
      );
      set({ missingDirs: status.missing, substitutedDirs: status.substituted });
    } catch (error) {
      log.error("presence check failed", toErrorFields(error));
    }
  },
}));
