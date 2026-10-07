// The Settings surface over durable configuration, including the sound and
// volume settings. This store owns only the draft, field validation, and
// picker; the workflow publishes only changed config sets.

import { create } from "zustand";
import {
  normalizeLanguagePreference,
  type LanguagePreference,
} from "../i18n/languages";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { log, toErrorFields } from "../repositories";
import { clampPlaybackVolume } from "../models/playback";
import { stringArrayField } from "../utils/configProjection";
import {
  configFlag,
  configNumber,
  configString,
  confirmsTrashDelete,
  type AppConfig,
} from "../models/config";
import { normalizeUiFontPreference } from "../utils/uiFont";
import { message, type Message } from "../i18n/translate";
import { recordActionFailure } from "./notifications-store";
import type { AiAccelerationCapability } from "../repositories";

export interface SettingsDraft {
  ignoredFileNames: string[];
  hideDotNames: boolean;
  hideHiddenAttributes: boolean;
  hideSystemAttributes: boolean;
  defaultTimezone: string;
  videoSnapshotsEnabled: boolean;
  similarPhotoAnalysisEnabled: boolean;
  videoTranscriptionEnabled: boolean;
  audioTranscriptionEnabled: boolean;
  aiAcceleration: Record<string, string>;
  autoplay: boolean;
  similarPhotoGrouping: "stricter" | "normal" | "looser";
  soundEnabled: boolean;
  playbackVolume: number;
  enlargeSmallImages: boolean;
  textFallbackEncoding: string;
  theme: "system" | "light" | "dark";
  language: LanguagePreference;
  uiFontFamily: string;
  keepAwakeDuringIndexing: boolean;
  checkGithubReleasesAtLaunch: boolean;
  checkSourceFoldersAtLaunch: boolean;
  confirmTrashDelete: boolean;
  scoreFaces: boolean;
  maximumImagesInComparison: number;
  notificationDisplaySeconds: number;
  sourceDirs: string[];
}

// The configuration arrives as the core's effective values (see
// models/config.ts), so the draft reads each member as it is and validates
// its shape; it supplies no defaults of its own.
function numberField(config: AppConfig | null, key: string): number {
  const value = configNumber(config, key);
  if (value === null) throw new Error(`${key} must be a number.`);
  return value;
}

function stringField(config: AppConfig | null, key: string): string {
  const value = configString(config, key);
  if (value === null) throw new Error(`${key} must be text.`);
  return value;
}

function draftFrom(
  config: AppConfig | null,
  accelerationCapabilities: AiAccelerationCapability[],
): SettingsDraft {
  if (config?.ignoredFileNames !== undefined && (!Array.isArray(config.ignoredFileNames)
    || config.ignoredFileNames.some((name) => typeof name !== "string"))) {
    throw new Error("Ignored file names must be a list of names.");
  }
  for (const key of ["hideDotNames", "hideHiddenAttributes", "hideSystemAttributes"]) {
    if (config?.[key] !== undefined && typeof config[key] !== "boolean") throw new Error(`${key} must be a boolean.`);
  }
  const storedAcceleration =
    config?.aiAcceleration &&
    typeof config.aiAcceleration === "object" &&
    !Array.isArray(config.aiAcceleration)
      ? (config.aiAcceleration as Record<string, unknown>)
      : {};
  const aiAcceleration: Record<string, string> = {};
  for (const capability of accelerationCapabilities) {
    const stored = storedAcceleration[capability.feature];
    aiAcceleration[capability.feature] =
      typeof stored === "string" ? stored : capability.default;
  }
  const flag = (key: string) => configFlag(config, key);
  return {
    ignoredFileNames: stringArrayField(config, "ignoredFileNames"),
    hideDotNames: flag("hideDotNames"),
    hideHiddenAttributes: flag("hideHiddenAttributes"),
    hideSystemAttributes: flag("hideSystemAttributes"),
    defaultTimezone: stringField(config, "defaultTimezone"),
    videoSnapshotsEnabled: flag("videoSnapshotsEnabled"),
    similarPhotoAnalysisEnabled: flag("similarPhotoAnalysisEnabled"),
    videoTranscriptionEnabled: flag("videoTranscriptionEnabled"),
    audioTranscriptionEnabled: flag("audioTranscriptionEnabled"),
    aiAcceleration,
    autoplay: flag("autoplay"),
    similarPhotoGrouping: config?.similarPhotoGrouping === "stricter" || config?.similarPhotoGrouping === "looser" ? config.similarPhotoGrouping : "normal",
    soundEnabled: flag("soundEnabled"),
    playbackVolume: clampPlaybackVolume(config?.playbackVolume),
    enlargeSmallImages: flag("enlargeSmallImages"),
    textFallbackEncoding: stringField(config, "textFallbackEncoding"),
    theme:
      config?.theme === "light" || config?.theme === "dark"
        ? config.theme
        : "system",
    language: normalizeLanguagePreference(config?.language),
    uiFontFamily: normalizeUiFontPreference(config?.uiFontFamily),
    keepAwakeDuringIndexing: flag("keepAwakeDuringIndexing"),
    checkGithubReleasesAtLaunch: flag("checkGithubReleasesAtLaunch"),
    checkSourceFoldersAtLaunch: flag("checkSourceFoldersAtLaunch"),
    confirmTrashDelete: confirmsTrashDelete(config),
    scoreFaces: flag("scoreFaces"),
    maximumImagesInComparison: Math.max(
      2,
      Math.floor(numberField(config, "maximumImagesInComparison")),
    ),
    notificationDisplaySeconds: Math.min(
      60,
      Math.max(1, numberField(config, "notificationDisplaySeconds")),
    ),
    sourceDirs: stringArrayField(config, "sourceDirs"),
  };
}

interface SettingsState {
  draft: SettingsDraft | null;
  accelerationCapabilities: AiAccelerationCapability[];
  /** The core's defaults for a new installation, for the reset action. */
  defaults: AppConfig | null;
  /** The draft as it was when the modal opened — the dirty-check baseline. */
  opened: SettingsDraft | null;
  saving: boolean;
  message: Message | null;
  messageLevel: "error" | "info" | null;
  beginEditing: (
    config: AppConfig | null,
    accelerationCapabilities?: AiAccelerationCapability[],
    defaults?: AppConfig | null,
  ) => void;
  discardDraft: () => void;
  update: (patch: Partial<SettingsDraft>) => void;
  addSourceDir: () => Promise<void>;
  removeSourceDir: (path: string) => void;
}


export const useSettingsStore = create<SettingsState>((set, get) => ({
  draft: null,
  accelerationCapabilities: [],
  defaults: null,
  opened: null,
  saving: false,
  message: null,
  messageLevel: null,

  beginEditing: (config, accelerationCapabilities = [], defaults = null) => {
    set({
      accelerationCapabilities,
      defaults,
      draft: draftFrom(config, accelerationCapabilities),
      opened: draftFrom(config, accelerationCapabilities),
      message: null,
      messageLevel: null,
    });
  },

  discardDraft: () => {
    if (get().saving) return;
    set({ draft: null, opened: null, accelerationCapabilities: [], defaults: null });
  },

  update: (patch) => {
    const draft = get().draft;
    if (draft) set({ draft: { ...draft, ...patch } });
  },

  addSourceDir: async () => {
    try {
      const picked = await openDialog({ directory: true, multiple: true });
      const paths = (
        Array.isArray(picked) ? picked : picked ? [picked] : []
      ).filter((p): p is string => typeof p === "string");
      const draft = get().draft;
      if (!draft) return;
      const merged = [...draft.sourceDirs];
      for (const path of paths) if (!merged.includes(path)) merged.push(path);
      get().update({ sourceDirs: merged });
    } catch (error) {
      log.error("settings source dir picker failed", toErrorFields(error));
      const failure = message("settings.directoryPickerFailed");
      set({ message: failure, messageLevel: "error" });
      recordActionFailure("source-picker-failed", failure, error);
    }
  },

  removeSourceDir: (path) => {
    const draft = get().draft;
    if (draft)
      get().update({ sourceDirs: draft.sourceDirs.filter((d) => d !== path) });
  },
}));


export function changedSettingsSets(draft: SettingsDraft, opened: SettingsDraft | null): AppConfig {
  const sets: AppConfig = { ...draft };
  const baseline = opened;
  return Object.fromEntries(Object.entries(sets).filter(([key, value]) => !baseline || JSON.stringify(value) !== JSON.stringify(baseline[key as keyof SettingsDraft])));
}
