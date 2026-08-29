// First-run wizard state + the volume-presence gate. The wizard opens when no
// source directories are configured (the inbox-zero handler has nothing to
// handle without them); the gate blocks work mode when configured directories
// are absent (an unmounted volume manifests as a missing directory).

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { log, toErrorFields } from "../repositories";
import { stringArrayField } from "../utils/configProjection";
import { requestSeq } from "./request-seq";

export interface WizardDir {
  path: string;
}

interface WizardState {
  open: boolean;
  step: 1 | 2;
  dirs: WizardDir[];
  timezone: string;
  timezoneValid: boolean;
  timezonePending: boolean;
  error: string | null;
  /** True when the wizard was RE-RUN over an existing setup. A first run has
   * nothing to return to, so only a re-run offers Cancel. */
  reconfigure: boolean;
  missingDirs: string[];
  substitutedDirs: string[];
  init: (config: Record<string, unknown> | null) => Promise<void>;
  /** Re-runs the wizard as RECONFIGURE: seeded from the current config, never
   * from empty — the only trigger a first-run wizard has after first run. */
  reopen: (config: Record<string, unknown> | null) => void;
  addDirs: () => Promise<void>;
  removeDir: (path: string) => void;
  setStep: (step: 1 | 2) => void;
  setTimezone: (name: string) => Promise<void>;
  /** Abandons a re-run, changing nothing. Never available on a first run. */
  cancel: () => void;
  recheckPresence: () => Promise<void>;
}

const timezoneValidation = requestSeq();

export const useWizardStore = create<WizardState>((set, get) => ({
  open: false,
  step: 1,
  dirs: [],
  timezone: "",
  timezoneValid: true,
  timezonePending: false,
  error: null,
  reconfigure: false,
  missingDirs: [],
  substitutedDirs: [],

  init: async (config) => {
    timezoneValidation.begin();
    const sourceDirs = stringArrayField(config, "sourceDirs");
    const timezone =
      typeof config?.defaultTimezone === "string" ? config.defaultTimezone : "UTC";
    if (sourceDirs.length === 0) {
      set({ open: true, step: 1, dirs: [], timezone, timezoneValid: true, timezonePending: false, error: null, reconfigure: false });
    } else {
      set({ open: false, timezone, timezoneValid: true, timezonePending: false, error: null, reconfigure: false });
      await get().recheckPresence();
    }
  },

  reopen: (config) => {
    timezoneValidation.begin();
    const sourceDirs = stringArrayField(config, "sourceDirs");
    const timezone =
      typeof config?.defaultTimezone === "string" ? config.defaultTimezone : "UTC";
    set({
      open: true,
      step: 1,
      timezone,
      timezoneValid: true,
      timezonePending: false,
      error: null,
      reconfigure: true,
      dirs: sourceDirs.map((path) => ({ path })),
    });
  },

  addDirs: async () => {
    set({ error: null });
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
      set({ error: "Couldn’t open the directory picker." });
    }
  },

  removeDir: (path) => {
    set({ dirs: get().dirs.filter((d) => d.path !== path) });
  },

  setStep: (step) => set({ step }),

  setTimezone: async (name) => {
    const fresh = timezoneValidation.begin();
    set({ timezone: name, timezoneValid: false, timezonePending: true, error: null });
    if (name.trim() === "") {
      if (fresh()) set({ timezonePending: false });
      return;
    }
    try {
      const valid = await invoke<boolean>("validate_timezone", { name });
      if (fresh()) set({ timezoneValid: valid, timezonePending: false });
    } catch (error) {
      log.error("wizard timezone validation failed", toErrorFields(error));
      if (fresh()) {
        set({
          timezoneValid: false,
          timezonePending: false,
          error: "Couldn’t check this timezone.",
        });
      }
    }
  },

  cancel: () => {
    // Nothing was written on the way through — every step edits store state
    // only, and the Finish workflow is the sole writer — so abandoning is
    // just a close.
    set({ open: false, reconfigure: false });
  },

  recheckPresence: async () => {
    try {
      const status = await invoke<{ missing: string[]; substituted: string[] }>(
        "check_source_dirs",
      );
      set({ missingDirs: status.missing, substitutedDirs: status.substituted, error: null });
    } catch (error) {
      log.error("presence check failed", toErrorFields(error));
      set({ error: "Couldn’t check the configured source folders." });
    }
  },
}));
