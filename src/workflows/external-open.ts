import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";

/** Opens only an indexed identity and permanently pauses any in-app session
 * for it before native delegation begins. */
export async function openInDefaultApp(
  hash: string | null,
  pathId: number | null,
): Promise<void> {
  const key = hash ?? (pathId === null ? null : `path-${pathId}`);
  if (key !== null) await emit("playback://pause", { key });
  await invoke("open_item_externally", {
    hash,
    pathId: hash === null ? pathId : null,
  });
}
