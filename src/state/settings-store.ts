// The settings surface over the Design's tunables. The store never validates
// beyond field-level checks (the resolver and features own semantics); Save
// merges into the existing config, persists, re-resolves the whole index from
// stored evidence (pure DB work — no file reads), and refreshes the views.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { log, toErrorFields } from "../repositories";

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
  scenesGridColumns: number;
  scenesGridRows: number;
  pairingEnabled: boolean;
  theme: "system" | "light" | "dark";
  uiFontFamily: string;
  keepAwakeDuringIndexing: boolean;
  verifyAfterCopy: boolean;
  confirmTrashDelete: boolean;
  scoreFaces: boolean;
  cacheDir: string | null;
  sourceDirs: string[];
}

function numberOr(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function draftFrom(config: Record<string, unknown> | null): SettingsDraft {
  return {
    defaultTimezone:
      typeof config?.defaultTimezone === "string" ? config.defaultTimezone : "UTC",
    goodRangeStartYear: numberOr(config?.goodRangeStartYear, 1995),
    similarityMaxGapSeconds: numberOr(config?.similarityMaxGapSeconds, 90),
    similarityPhashMaxDistance: numberOr(config?.similarityPhashMaxDistance, 3),
    similarityPhashMaxDistanceBurst: numberOr(config?.similarityPhashMaxDistanceBurst, 10),
    similarityDiameterMultiplier: numberOr(config?.similarityDiameterMultiplier, 2),
    previewLongEdgePx: numberOr(config?.previewLongEdgePx, 1600),
    thumbnailEdgePx: numberOr(config?.thumbnailEdgePx, 320),
    videoStripSecondsPerFrame: numberOr(config?.videoStripSecondsPerFrame, 20),
    videoStripMinFrames: numberOr(config?.videoStripMinFrames, 5),
    videoStripMaxFrames: numberOr(config?.videoStripMaxFrames, 40),
    scenesGridColumns: numberOr(config?.scenesGridColumns, 6),
    scenesGridRows: numberOr(config?.scenesGridRows, 4),
    pairingEnabled: config?.pairingEnabled !== false,
    theme:
      config?.theme === "light" || config?.theme === "dark" ? config.theme : "system",
    uiFontFamily: typeof config?.uiFontFamily === "string" ? config.uiFontFamily : "",
    keepAwakeDuringIndexing: config?.keepAwakeDuringIndexing !== false,
    verifyAfterCopy: config?.verifyAfterCopy !== false,
    // Opt-in, so absence means OFF — the opposite polarity of the two above.
    confirmTrashDelete: config?.confirmTrashDelete === true,
    // Opt-in (Phase 33): absence means OFF.
    scoreFaces: config?.scoreFaces === true,
    cacheDir:
      typeof config?.cacheDir === "string" && config.cacheDir.trim() !== ""
        ? config.cacheDir
        : null,
    sourceDirs: Array.isArray(config?.sourceDirs) ? (config.sourceDirs as string[]) : [],
  };
}

interface SettingsState {
  open: boolean;
  draft: SettingsDraft | null;
  /** The draft as it was when the modal opened — the dirty-check baseline. */
  opened: SettingsDraft | null;
  timezoneValid: boolean;
  saving: boolean;
  /** Non-null while a cache move runs — drives the blocking progress surface. */
  movingCache: { copiedBytes: number; totalBytes: number } | null;
  cancellingCacheMove: boolean;
  message: string;
  openWith: (config: Record<string, unknown> | null) => void;
  close: () => void;
  update: (patch: Partial<SettingsDraft>) => void;
  validateTimezone: (name: string) => Promise<void>;
  addSourceDir: () => Promise<void>;
  removeSourceDir: (path: string) => void;
  pickCacheDir: () => Promise<void>;
  clearCacheDir: () => void;
  cancelCacheMove: () => Promise<void>;
  save: () => Promise<void>;
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  open: false,
  draft: null,
  opened: null,
  timezoneValid: true,
  saving: false,
  movingCache: null,
  cancellingCacheMove: false,
  message: "",

  openWith: (config) =>
    set({
      open: true,
      draft: draftFrom(config),
      opened: draftFrom(config),
      timezoneValid: true,
      message: "",
    }),

  close: () => {
    if (get().saving) return;
    set({ open: false, draft: null, opened: null });
  },

  update: (patch) => {
    const draft = get().draft;
    if (draft) set({ draft: { ...draft, ...patch } });
  },

  validateTimezone: async (name) => {
    get().update({ defaultTimezone: name });
    try {
      const valid = await invoke<boolean>("validate_timezone", { name });
      if (get().draft?.defaultTimezone === name) set({ timezoneValid: valid });
    } catch {
      set({ timezoneValid: false });
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

  pickCacheDir: async () => {
    try {
      const picked = await openDialog({ directory: true, multiple: false });
      if (typeof picked === "string") get().update({ cacheDir: picked });
    } catch (error) {
      log.error("settings cache dir picker failed", toErrorFields(error));
    }
  },

  clearCacheDir: () => get().update({ cacheDir: null }),

  cancelCacheMove: async () => {
    if (get().movingCache === null || get().cancellingCacheMove) return;
    set({ cancellingCacheMove: true });
    try {
      const active = await invoke<boolean>("cancel_cache_move");
      if (!active) set({ cancellingCacheMove: false });
    } catch (error) {
      set({ cancellingCacheMove: false });
      log.error("cache move cancellation failed", toErrorFields(error));
    }
  },

  save: async () => {
    const { draft, opened, timezoneValid } = get();
    if (!draft || !timezoneValid) return;
    set({ saving: true, message: "" });
    try {
      // A changed cache location is a REAL move (the developer's contract):
      // blocking progress surface, copy → verify → swap → delete old, the
      // core patches cacheDir itself only after the copy verified. A failed
      // move keeps the old location live and aborts the whole save.
      if (draft.cacheDir !== (opened?.cacheDir ?? null)) {
        const { listen } = await import("@tauri-apps/api/event");
        set({ movingCache: { copiedBytes: 0, totalBytes: 0 }, cancellingCacheMove: false });
        const unlisten = await listen<{ copiedBytes: number; totalBytes: number }>(
          "cache-move://progress",
          (event) => set({ movingCache: event.payload }),
        );
        try {
          await invoke("move_cache", { newDir: draft.cacheDir });
        } finally {
          unlisten();
          set({ movingCache: null, cancellingCacheMove: false });
        }
      }
      // A patch of exactly the draft's keys through the one config owner —
      // keys other surfaces manage (destinationRoots) are never touched.
      // cacheDir stays out: the move above already committed it, and a
      // failed move must not be resurrected by the patch.
      const { cacheDir: _committedByMove, ...rest } = draft;
      const { useAppStore } = await import("./app-store");
      await useAppStore.getState().patchConfig({ ...rest });
      // Settings changes re-resolve everything from stored evidence and
      // rebuild groups — pure DB work, no file reads.
      const resolved = await invoke<number>("re_resolve_all");
      const { useSectionsStore } = await import("./sections-store");
      await useSectionsStore.getState().loadCounts();
      const { useItemsStore } = await import("./items-store");
      await useItemsStore.getState().refresh();
      const { useWizardStore } = await import("./wizard-store");
      await useWizardStore.getState().recheckPresence();
      set({ open: false, draft: null, opened: null, saving: false });
      log.info("settings saved", { resolved });
    } catch (error) {
      const message = String(error);
      if (message.includes("cache move cancelled")) {
        set({ saving: false, message: "Cache move cancelled." });
        log.info("cache move cancelled", {});
      } else {
        set({ saving: false, message });
        log.error("settings save failed", toErrorFields(error));
      }
    }
  },
}));
