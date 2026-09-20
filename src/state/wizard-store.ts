// First-run wizard state + configured-source availability. The wizard opens
// when no source directories are configured. Missing roots remain visible but
// do not block Main; only a substituted physical volume is an unsafe gate.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { log, toErrorFields } from "../repositories";
import { stringArrayField } from "../utils/configProjection";
import { requestSeq } from "./request-seq";
import { message, type Message } from "../i18n/translate";
import { recordActionFailure } from "./notifications-store";
import {
  effectiveLanguage,
  normalizeLanguagePreference,
  type Language,
  type LanguagePreference,
} from "../i18n/languages";
import { useLanguageStore } from "./language-store";
import {
  optionalFeatureSetup,
  type OptionalFeatureChoices,
  type OptionalFeatureId,
} from "../models/optionalFeatures";

export interface WizardDir {
  path: string;
}

interface WizardState {
  open: boolean;
  step: 1 | 2 | 3;
  dirs: WizardDir[];
  /** The staged interface language. Like every other wizard answer it is
   * written only by Finish, but the wizard shows it at once so the reader sees
   * the language they picked while answering. */
  language: LanguagePreference;
  /** The language in effect when the wizard opened, so abandoning a re-run
   * puts the preview back. */
  languageBefore: Language;
  timezone: string;
  error: Message | null;
  finishing: boolean;
  /** True when the wizard was RE-RUN over an existing setup. A first run has
   * nothing to return to, so only a re-run offers Cancel. */
  reconfigure: boolean;
  optionalFeatures: OptionalFeatureChoices;
  missingDirs: string[];
  substitutedDirs: string[];
  init: (config: Record<string, unknown> | null) => Promise<void>;
  /** Re-runs the wizard as RECONFIGURE: seeded from the current config, never
   * from empty — the only trigger a first-run wizard has after first run. */
  reopen: (config: Record<string, unknown> | null) => void;
  addDirs: () => Promise<void>;
  removeDir: (path: string) => void;
  setStep: (step: 1 | 2 | 3) => void;
  setOptionalFeature: (id: OptionalFeatureId, enabled: boolean) => void;
  setTimezone: (name: string) => void;
  setLanguage: (preference: LanguagePreference) => void;
  /** Abandons a re-run, changing nothing. Never available on a first run. */
  cancel: () => void;
  recheckPresence: () => Promise<void>;
}

const presenceCheck = requestSeq();

export const useWizardStore = create<WizardState>((set, get) => ({
  open: false,
  step: 1,
  dirs: [],
  language: "system",
  languageBefore: "en",
  timezone: "",
  error: null,
  finishing: false,
  reconfigure: false,
  optionalFeatures: optionalFeatureSetup(null),
  missingDirs: [],
  substitutedDirs: [],

  init: async (config) => {
    const sourceDirs = stringArrayField(config, "sourceDirs");
    const timezone =
      typeof config?.defaultTimezone === "string" ? config.defaultTimezone : "UTC";
    const language = normalizeLanguagePreference(config?.language);
    if (sourceDirs.length === 0) {
      presenceCheck.begin();
      set({
        open: true,
        step: 1,
        dirs: [],
        language,
        languageBefore: useLanguageStore.getState().language,
        timezone,
        error: null,
        finishing: false,
        reconfigure: false,
        optionalFeatures: optionalFeatureSetup(config),
        missingDirs: [],
        substitutedDirs: [],
      });
    } else {
      set({
        open: false,
        language,
        timezone,
        error: null,
        finishing: false,
        reconfigure: false,
        missingDirs: [],
        substitutedDirs: [],
      });
      await get().recheckPresence();
    }
  },

  reopen: (config) => {
    const sourceDirs = stringArrayField(config, "sourceDirs");
    const timezone =
      typeof config?.defaultTimezone === "string" ? config.defaultTimezone : "UTC";
    set({
      open: true,
      step: 1,
      language: normalizeLanguagePreference(config?.language),
      languageBefore: useLanguageStore.getState().language,
      timezone,
      error: null,
      finishing: false,
      reconfigure: true,
      dirs: sourceDirs.map((path) => ({ path })),
      optionalFeatures: optionalFeatureSetup(config),
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
      const failure = message("settings.directoryPickerFailed");
      set({ error: failure });
      recordActionFailure("setup-source-picker-failed", failure, error);
    }
  },

  removeDir: (path) => {
    set({ dirs: get().dirs.filter((d) => d.path !== path) });
  },

  setStep: (step) => set({ step }),

  setOptionalFeature: (id, enabled) => {
    set({ optionalFeatures: { ...get().optionalFeatures, [id]: enabled } });
  },

  setTimezone: (name) => {
    // The zone comes from the list, so there is nothing to validate; it is
    // still staged like every other wizard answer and written only by Finish.
    set({ timezone: name, error: null });
  },

  setLanguage: (preference) => {
    set({ language: preference });
    const { systemLanguage } = useLanguageStore.getState();
    useLanguageStore.setState({ language: effectiveLanguage(preference, systemLanguage) });
  },

  cancel: () => {
    // Nothing was written on the way through — every step edits store state
    // only, and the Finish workflow is the sole writer — so abandoning is
    // just a close. The previewed language goes back with it.
    useLanguageStore.setState({ language: get().languageBefore });
    set({ open: false, reconfigure: false });
  },

  recheckPresence: async () => {
    const fresh = presenceCheck.begin();
    try {
      const status = await invoke<{ missing: string[]; substituted: string[] }>(
        "check_source_dirs",
      );
      if (fresh()) {
        set({ missingDirs: status.missing, substitutedDirs: status.substituted, error: null });
      }
    } catch (error) {
      if (!fresh()) return;
      log.error("presence check failed", toErrorFields(error));
      const failure = message("wizard.sourceCheckFailed");
      set({ error: failure });
      recordActionFailure("configured-source-check-failed", failure, error);
    }
  },
}));
