import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { recordActionFailure } from "../state/notifications-store";

/** Opens only an indexed identity and permanently pauses any in-app session
 * for it before native delegation begins. */
export async function openInDefaultApp(
  hash: string | null,
  pathId: number | null,
): Promise<void> {
  const key = hash ?? (pathId === null ? null : `path-${pathId}`);
  if (key !== null) await emit("playback://pause", { key });
  try {
    await invoke("open_item_externally", {
      hash,
      pathId: hash === null ? pathId : null,
    });
  } catch (error) {
    recordActionFailure(
      "external-open-failed",
      "Couldn’t open this file in its default app.",
      error,
    );
    throw error;
  }
}

export async function revealInFileManager(path: string): Promise<void> {
  try {
    await revealItemInDir(path);
  } catch (error) {
    recordActionFailure(
      "reveal-file-failed",
      "Couldn’t reveal this file in the file manager.",
      error,
    );
    throw error;
  }
}
