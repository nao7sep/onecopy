import { invoke } from "@tauri-apps/api/core";

/** The core preserves native directory identity, including non-Unicode names. */
export function pickDirectories(multiple: boolean): Promise<string[]> {
  return invoke<string[]>("pick_directories", { multiple });
}
