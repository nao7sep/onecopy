// Managed-dependency state: one entry per registry row (the ffmpeg binary,
// the model files), the ffmpeg chip in the footer reading its entry, a named
// modal with one context-aware action per row, progress from the install
// events. No check ever runs automatically — the modal's buttons and the
// config-gated launch check are the only triggers (the honest-state model's
// UI counterpart).

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { log, toErrorFields } from "../repositories";

export type DependencyStatus =
  | "not-installed"
  | "update-available"
  | "up-to-date"
  | "installed-unchecked";

export interface DependencyState {
  id: string;
  label: string;
  kind: "binary" | "model";
  status: DependencyStatus;
  facts: {
    installedVersion: string | null;
    latestKnownVersion: string | null;
    lastCheckedAtUtc: string | null;
  };
  path: string;
}

/** The attention role a status carries in the UI (managed-runtime-dependencies
 * conventions): every entry here is OPTIONAL, so absent is informational — a
 * feature simply not set up — while an available update IS the warning. The
 * call site names the role; the theme maps roles to colors. */
export type FfmpegRole = "neutral" | "warning";
export function ffmpegRole(status: DependencyStatus | null): FfmpegRole {
  return status === "update-available" ? "warning" : "neutral";
}

interface BinariesState {
  entries: DependencyState[];
  /** The id currently installing, with its live progress line. */
  installingId: string | null;
  progress: string;
  modalOpen: boolean;
  load: () => Promise<void>;
  install: (id: string) => Promise<void>;
  check: (id: string) => Promise<void>;
  setModalOpen: (open: boolean) => void;
}

export const useBinariesStore = create<BinariesState>((set) => ({
  entries: [],
  installingId: null,
  progress: "",
  modalOpen: false,

  load: async () => {
    try {
      const entries = await invoke<DependencyState[]>("binaries_state");
      set({ entries: Array.isArray(entries) ? entries : [] });
    } catch (error) {
      log.error("binaries state load failed", toErrorFields(error));
    }
  },

  install: async (id) => {
    try {
      set({ installingId: id, progress: "starting…" });
      await invoke("binaries_install", { id });
    } catch (error) {
      set({ installingId: null, progress: "" });
      log.error("binaries install start failed", toErrorFields(error));
    }
  },

  check: async (id) => {
    try {
      set({ entries: await invoke<DependencyState[]>("binaries_check", { id }) });
    } catch (error) {
      log.error("binaries check failed", toErrorFields(error));
    }
  },

  setModalOpen: (open) => set({ modalOpen: open }),
}));

/** The ffmpeg entry — the chip, the wizard offer, and the scan honesty all
 * read this one row. */
export function ffmpegEntry(
  entries: DependencyState[] | null | undefined,
): DependencyState | null {
  return entries?.find((entry) => entry.id === "ffmpeg") ?? null;
}

void (async () => {
  try {
    await listen<{ id: string; phase: string; detail: string }>(
      "binaries://progress",
      (event) => {
        useBinariesStore.setState({
          installingId: event.payload.id,
          progress: `${event.payload.phase}: ${event.payload.detail}`,
        });
      },
    );
    await listen("binaries://done", () => {
      useBinariesStore.setState({ installingId: null, progress: "" });
      void useBinariesStore.getState().load();
    });
    await listen<{ id: string; message: string }>("binaries://error", (event) => {
      useBinariesStore.setState({
        installingId: null,
        progress: `failed: ${event.payload.message}`,
      });
      void useBinariesStore.getState().load();
    });
    // The launch-time update check (config-gated, core-side) finished after
    // this store's initial load — refresh so the chip reflects it.
    await listen("binaries://changed", () => {
      void useBinariesStore.getState().load();
    });
  } catch (error) {
    log.warn("binaries event wiring failed", toErrorFields(error));
  }
})();

/** What the footer says about ffmpeg, or null to say nothing at all.
 *
 * The managed-runtime-dependencies conventions make **Up to date** a silent
 * default and warn against permanent benign FYIs, so a working install shows
 * nothing. **Installed (not checked)** is silent for the same reason — with
 * update checks off by default it would otherwise be a permanent fixture.
 * **Not installed** deliberately stays visible: the convention notes that
 * silence for an optional-absent dependency "risks a dead feature", and here
 * that dead feature is every video and every HEIC photo. The chip covers
 * ffmpeg alone — a missing MODEL disables one enhancement, not a media kind,
 * and its honest surface is the feature's own control naming the remedy.
 */
export function ffmpegChipText(
  installing: boolean,
  progress: string,
  status: string | null,
): string | null {
  if (installing) return progress;
  switch (status) {
    case "not-installed":
      return "ffmpeg not installed";
    case "update-available":
      return "ffmpeg update available";
    default:
      // up-to-date, installed-unchecked, and the pre-load null.
      return null;
  }
}
