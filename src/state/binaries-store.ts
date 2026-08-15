// The managed-ffmpeg state: a status chip in the footer, a named modal with
// the one context-aware action, progress from the install events. No check
// ever runs automatically — the modal's button is the only trigger (the
// honest-state model's UI counterpart).

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { log, toErrorFields } from "../repositories";

export interface FfmpegState {
  status: "not-installed" | "update-available" | "up-to-date" | "installed-unchecked";
  facts: {
    installedVersion: string | null;
    latestKnownVersion: string | null;
    lastCheckedAtUtc: string | null;
  };
  path: string;
}

/** The attention role a status carries in the UI (managed-runtime-dependencies
 * conventions): ffmpeg is OPTIONAL here, so absent is informational — a
 * feature simply not set up — while an available update IS the warning. The
 * call site names the role; the theme maps roles to colors. */
export type FfmpegRole = "neutral" | "warning";
export function ffmpegRole(status: FfmpegState["status"] | null): FfmpegRole {
  return status === "update-available" ? "warning" : "neutral";
}

interface BinariesState {
  state: FfmpegState | null;
  installing: boolean;
  progress: string;
  modalOpen: boolean;
  load: () => Promise<void>;
  install: () => Promise<void>;
  check: () => Promise<void>;
  setModalOpen: (open: boolean) => void;
}

export const useBinariesStore = create<BinariesState>((set) => ({
  state: null,
  installing: false,
  progress: "",
  modalOpen: false,

  load: async () => {
    try {
      set({ state: await invoke<FfmpegState>("binaries_state") });
    } catch (error) {
      log.error("binaries state load failed", toErrorFields(error));
    }
  },

  install: async () => {
    try {
      set({ installing: true, progress: "starting…" });
      await invoke("binaries_install");
    } catch (error) {
      set({ installing: false, progress: "" });
      log.error("binaries install start failed", toErrorFields(error));
    }
  },

  check: async () => {
    try {
      set({ state: await invoke<FfmpegState>("binaries_check") });
    } catch (error) {
      log.error("binaries check failed", toErrorFields(error));
    }
  },

  setModalOpen: (open) => set({ modalOpen: open }),
}));

void (async () => {
  try {
    await listen<{ phase: string; detail: string }>("binaries://progress", (event) => {
      useBinariesStore.setState({
        installing: true,
        progress: `${event.payload.phase}: ${event.payload.detail}`,
      });
    });
    await listen("binaries://done", () => {
      useBinariesStore.setState({ installing: false, progress: "" });
      void useBinariesStore.getState().load();
    });
    await listen<{ message: string }>("binaries://error", (event) => {
      useBinariesStore.setState({
        installing: false,
        progress: `failed: ${event.payload.message}`,
      });
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
 * that dead feature is every video and every HEIC photo.
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
