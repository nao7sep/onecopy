import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { Monitor } from "@tauri-apps/api/window";
import { waitForWindowCreated } from "../utils/windowCreation";
import { toErrorFields } from "../repositories";

let pending: Promise<void> | null = null;

/** Creation is one bounded operation; each flash owns its later self-close.
 * Native identities cannot collide with a previous batch still closing. */
export function identifyScreens(monitors: readonly Monitor[]): Promise<void> {
  if (pending !== null) return pending;
  const batch = crypto.randomUUID();
  const operation = Promise.allSettled(monitors.map(async (monitor, index) => {
    const scale = monitor.scaleFactor || 1;
    const window = new WebviewWindow(`identify-${batch}-${index + 1}`, {
      url: `index.html?view=identify&slice=${index + 1}`,
      title: "OneCopy",
      x: (monitor.position.x + monitor.size.width / 2) / scale - 110,
      y: (monitor.position.y + monitor.size.height / 2) / scale - 110,
      width: 220,
      height: 220,
      decorations: false,
      alwaysOnTop: true,
      skipTaskbar: true,
      resizable: false,
      focus: false,
    });
    await waitForWindowCreated(window, `Screen ${index + 1} identification`);
  })).then((results) => {
    const failures = results.filter((result) => result.status === "rejected");
    if (failures.length > 0) {
      throw Object.assign(new Error("Screen identification failed"), {
        cause: failures.map((failure) => toErrorFields(failure.reason)),
      });
    }
  });
  pending = operation.finally(() => { pending = null; });
  return pending;
}
