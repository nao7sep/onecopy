// The preview window (screen 2 in the screen-priority model): created on
// demand, positioned on the second monitor when one exists, and following the
// main window's selection while open. The main window emits `preview://show`
// with just the item key; the preview window fetches what it needs itself.

import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { availableMonitors } from "@tauri-apps/api/window";
import { emit } from "@tauri-apps/api/event";
import { log, toErrorFields } from "../repositories";

export interface PreviewPayload {
  hash: string | null;
  pathId: number | null;
}

async function positionOnSecondMonitor(window: WebviewWindow): Promise<void> {
  try {
    const monitors = await availableMonitors();
    if (monitors.length < 2) return;
    const second = monitors[1];
    await window.setPosition(second.position);
    await window.setFocus();
  } catch (error) {
    log.warn("preview window placement failed", toErrorFields(error));
  }
}

/** Opens (or reuses) the preview window and shows the item in it. */
export async function showPreview(payload: PreviewPayload): Promise<void> {
  try {
    let window = await WebviewWindow.getByLabel("preview");
    if (window === null) {
      window = new WebviewWindow("preview", {
        url: "index.html?view=preview",
        title: "OneCopy Preview",
        width: 1280,
        height: 800,
      });
      await new Promise<void>((resolve, reject) => {
        void window!.once("tauri://created", () => resolve());
        void window!.once("tauri://error", (e) => reject(e.payload));
      });
      await positionOnSecondMonitor(window);
    }
    // A brief delay is unnecessary: the payload is re-emitted on every
    // selection change, and the window asks for the current one on load.
    await emit("preview://show", payload);
  } catch (error) {
    log.error("preview window open failed", toErrorFields(error));
  }
}

/** Re-emits the selection to a preview window if one is open (live follow). */
export async function updatePreviewIfOpen(payload: PreviewPayload): Promise<void> {
  try {
    const window = await WebviewWindow.getByLabel("preview");
    if (window !== null) {
      await emit("preview://show", payload);
    }
  } catch {
    // No window, nothing to follow.
  }
}
