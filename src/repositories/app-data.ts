// Startup data + config/state persistence, all through Rust commands. The
// webview never resolves a data path or touches the filesystem; the core owns
// the storage root, the atomic writes, and the backup hook.

import { invoke } from "@tauri-apps/api/core";

// Mirrors storage::LoadedAppData. Config and state stay loosely typed at this
// layer (the store never validates; each feature validates what it consumes).
export interface LoadedAppData {
  config: Record<string, unknown> | null;
  state: Record<string, unknown> | null;
  dataRoot: string;
  debugEnabled: boolean;
  faceScoringSupported: boolean;
  transcriptionSupported: boolean;
  /** Stores the core set aside this launch because they would not parse. The
   * app must SHOW these — an unreported quarantine is a silent reset with
   * extra steps (storage-path-conventions). */
  quarantines: QuarantineRecord[];
}

/** Mirrors storage::QuarantineRecord. */
export interface QuarantineRecord {
  /** The store's file name — `config.json`, `state.json`. */
  file: string;
  /** Where the original bytes were moved, verbatim, so the user can go get
   * them. */
  quarantinedTo: string;
}

export function loadAppData(): Promise<LoadedAppData> {
  return invoke<LoadedAppData>("load_app_data");
}

// Saves are PATCHES merged core-side (the core holds the file and owns the
// read-modify-write); the merged document comes back so callers can publish
// it without a second read. Route through app-store's patchConfig/patchState
// so the one config/state owner stays current — never call these around it.

export function patchConfigFile(
  patch: Record<string, unknown>,
  reportFailure = true,
): Promise<Record<string, unknown>> {
  return invoke<Record<string, unknown>>("patch_config", { patch, reportFailure });
}

export function patchStateFile(
  patch: Record<string, unknown>,
): Promise<Record<string, unknown>> {
  return invoke<Record<string, unknown>>("patch_state", { patch });
}
