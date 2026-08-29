// The settings surface over the Design's tunables. This store owns the draft,
// field validation, and picker. The cross-store Save journey lives in
// workflows/settings.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { log, toErrorFields } from "../repositories";
import { stringArrayField } from "../utils/configProjection";
import { normalizeUiFontPreference } from "../utils/theme";
import { requestSeq } from "./request-seq";

export interface SettingsDraft {
  defaultTimezone: string;
  goodRangeStartYear: number;
  similarityMaxGapSeconds: number;
  similarityPhashMaxDistance: number;
  similarityPhashMaxDistanceBurst: number;
  similarityDiameterMultiplier: number;
  previewLongEdgePx: number;
  thumbnailEdgePx: number;
  videoStripSecondsPerFrame: number;
  videoStripMinFrames: number;
  videoStripMaxFrames: number;
  videoAutoplayOnShow: boolean;
  videoAutoplayAfterSnapshot: boolean;
  pairingEnabled: boolean;
  theme: "system" | "light" | "dark";
  uiFontFamily: string;
  keepAwakeDuringIndexing: boolean;
  checkSourceFoldersAtLaunch: boolean;
  confirmTrashDelete: boolean;
  scoreFaces: boolean;
  showFaceStars: boolean;
  sourceDirs: string[];
}

const SIMILAR_PHOTO_DEFAULTS = {
  similarityMaxGapSeconds: 90,
  similarityPhashMaxDistance: 3,
  similarityPhashMaxDistanceBurst: 10,
  similarityDiameterMultiplier: 2,
} as const satisfies Pick<
  SettingsDraft,
  | "similarityMaxGapSeconds"
  | "similarityPhashMaxDistance"
  | "similarityPhashMaxDistanceBurst"
  | "similarityDiameterMultiplier"
>;

function numberOr(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function draftFrom(config: Record<string, unknown> | null): SettingsDraft {
  return {
    defaultTimezone:
      typeof config?.defaultTimezone === "string" ? config.defaultTimezone : "UTC",
    goodRangeStartYear: numberOr(config?.goodRangeStartYear, 1995),
    similarityMaxGapSeconds: numberOr(
      config?.similarityMaxGapSeconds,
      SIMILAR_PHOTO_DEFAULTS.similarityMaxGapSeconds,
    ),
    similarityPhashMaxDistance: numberOr(
      config?.similarityPhashMaxDistance,
      SIMILAR_PHOTO_DEFAULTS.similarityPhashMaxDistance,
    ),
    similarityPhashMaxDistanceBurst: numberOr(
      config?.similarityPhashMaxDistanceBurst,
      SIMILAR_PHOTO_DEFAULTS.similarityPhashMaxDistanceBurst,
    ),
    similarityDiameterMultiplier: numberOr(
      config?.similarityDiameterMultiplier,
      SIMILAR_PHOTO_DEFAULTS.similarityDiameterMultiplier,
    ),
    previewLongEdgePx: numberOr(config?.previewLongEdgePx, 1600),
    thumbnailEdgePx: numberOr(config?.thumbnailEdgePx, 320),
    videoStripSecondsPerFrame: numberOr(config?.videoStripSecondsPerFrame, 20),
    videoStripMinFrames: numberOr(config?.videoStripMinFrames, 5),
    videoStripMaxFrames: numberOr(config?.videoStripMaxFrames, 40),
    videoAutoplayOnShow: config?.videoAutoplayOnShow !== false,
    videoAutoplayAfterSnapshot: config?.videoAutoplayAfterSnapshot !== false,
    pairingEnabled: config?.pairingEnabled !== false,
    theme:
      config?.theme === "light" || config?.theme === "dark" ? config.theme : "system",
    uiFontFamily: normalizeUiFontPreference(config?.uiFontFamily),
    keepAwakeDuringIndexing: config?.keepAwakeDuringIndexing !== false,
    checkSourceFoldersAtLaunch: config?.checkSourceFoldersAtLaunch !== false,
    // Opt-in, so absence means OFF — the opposite polarity of the two above.
    confirmTrashDelete: config?.confirmTrashDelete === true,
    // Opt-in (Phase 33): absence means OFF.
    scoreFaces: config?.scoreFaces === true,
    // Presentation is independent of scoring: existing results remain useful
    // after the optional background analysis is turned off.
    showFaceStars: config?.showFaceStars !== false,
    sourceDirs: stringArrayField(config, "sourceDirs"),
  };
}

interface SettingsState {
  open: boolean;
  draft: SettingsDraft | null;
  /** The draft as it was when the modal opened — the dirty-check baseline. */
  opened: SettingsDraft | null;
  timezoneValid: boolean;
  timezonePending: boolean;
  saving: boolean;
  message: string;
  openWith: (config: Record<string, unknown> | null) => void;
  close: () => void;
  update: (patch: Partial<SettingsDraft>) => void;
  resetSimilarPhotoSettings: () => void;
  validateTimezone: (name: string) => Promise<void>;
  addSourceDir: () => Promise<void>;
  removeSourceDir: (path: string) => void;
}

const timezoneValidation = requestSeq();

export const useSettingsStore = create<SettingsState>((set, get) => ({
  open: false,
  draft: null,
  opened: null,
  timezoneValid: true,
  timezonePending: false,
  saving: false,
  message: "",

  openWith: (config) => {
    timezoneValidation.begin();
    set({
      open: true,
      draft: draftFrom(config),
      opened: draftFrom(config),
      timezoneValid: true,
      timezonePending: false,
      message: "",
    });
  },

  close: () => {
    if (get().saving) return;
    set({ open: false, draft: null, opened: null });
  },

  update: (patch) => {
    const draft = get().draft;
    if (draft) set({ draft: { ...draft, ...patch } });
  },

  resetSimilarPhotoSettings: () => get().update(SIMILAR_PHOTO_DEFAULTS),

  validateTimezone: async (name) => {
    const fresh = timezoneValidation.begin();
    get().update({ defaultTimezone: name });
    set({ timezoneValid: false, timezonePending: true });
    if (name.trim() === "") {
      if (fresh()) set({ timezonePending: false });
      return;
    }
    try {
      const valid = await invoke<boolean>("validate_timezone", { name });
      if (fresh()) set({ timezoneValid: valid, timezonePending: false });
    } catch {
      if (fresh()) set({ timezoneValid: false, timezonePending: false });
    }
  },

  addSourceDir: async () => {
    try {
      const picked = await openDialog({ directory: true, multiple: true });
      const paths = (Array.isArray(picked) ? picked : picked ? [picked] : []).filter(
        (p): p is string => typeof p === "string",
      );
      const draft = get().draft;
      if (!draft) return;
      const merged = [...draft.sourceDirs];
      for (const path of paths) if (!merged.includes(path)) merged.push(path);
      get().update({ sourceDirs: merged });
    } catch (error) {
      log.error("settings source dir picker failed", toErrorFields(error));
    }
  },

  removeSourceDir: (path) => {
    const draft = get().draft;
    if (draft) get().update({ sourceDirs: draft.sourceDirs.filter((d) => d !== path) });
  },
}));
