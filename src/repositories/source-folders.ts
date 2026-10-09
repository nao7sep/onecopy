import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "./logging";

/** The picked folders the core refuses as source folders: those inside or
 * equal to a library or app package, whose files belong to the program that
 * made it. A failed answer refuses nothing here; saving refuses them again. */
export async function packageSourceDirs(paths: string[]): Promise<string[]> {
  if (paths.length === 0) return [];
  try {
    return await invoke<string[]>("package_source_dirs", { paths });
  } catch (error) {
    log.warn("package folder check failed", toErrorFields(error));
    return [];
  }
}
