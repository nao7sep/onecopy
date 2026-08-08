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
}

export function loadAppData(): Promise<LoadedAppData> {
  return invoke<LoadedAppData>("load_app_data");
}

export function saveConfig(config: Record<string, unknown>): Promise<void> {
  return invoke<void>("save_config", { config });
}

export function saveState(state: Record<string, unknown>): Promise<void> {
  return invoke<void>("save_state", { state });
}
