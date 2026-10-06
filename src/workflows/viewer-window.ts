import { invoke } from "@tauri-apps/api/core";
import {
  PhysicalPosition,
  PhysicalSize,
  availableMonitors,
  currentMonitor,
} from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { reportWindowCall } from "../repositories";
import { waitForWindowCreated } from "../utils/windowCreation";
import { documentTranslator } from "../i18n/I18nContext";

const VIEWER_LABEL = "viewer";

interface ViewerMonitor {
  position: { x: number; y: number };
  size: { width: number; height: number };
  scaleFactor: number;
}

let desired = false;
let lifecycle: Promise<void> = Promise.resolve();

function queue(action: () => Promise<void>): Promise<void> {
  const next = lifecycle.then(action, action);
  lifecycle = next.catch(() => undefined);
  return next;
}

function setFullscreen(enable: boolean): Promise<void> {
  return invoke<void>("set_window_fullscreen", { label: VIEWER_LABEL, enable });
}

async function resolveMonitor(): Promise<ViewerMonitor | null> {
  const hosting = await currentMonitor();
  if (hosting !== null) return hosting;
  return (await availableMonitors())[0] ?? null;
}

/** Placed on the display first, so fullscreen takes that display's frame. */
async function activate(window: WebviewWindow, monitor: ViewerMonitor): Promise<void> {
  await window.setPosition(new PhysicalPosition(monitor.position.x, monitor.position.y));
  await window.setSize(new PhysicalSize(monitor.size.width, monitor.size.height));
  await window.show();
  await setFullscreen(true);
  await window.setFocus();
}

async function createViewer(monitor: ViewerMonitor): Promise<WebviewWindow> {
  const scale = monitor.scaleFactor || 1;
  const window = new WebviewWindow(VIEWER_LABEL, {
    url: "index.html?view=viewer",
    // window-appearance.ts sets the definitive title on first paint; this is
    // only what shows for the brief instant before that (L6).
    title: documentTranslator().t("window.titleViewer"),
    x: monitor.position.x / scale,
    y: monitor.position.y / scale,
    width: monitor.size.width / scale,
    height: monitor.size.height / scale,
    decorations: false,
    skipTaskbar: true,
    resizable: false,
    focus: false,
    visible: false,
  });
  await waitForWindowCreated(window, "Viewer");
  return window;
}

async function enter(): Promise<void> {
  const monitor = await resolveMonitor();
  if (monitor === null) throw new Error("No display is available for fullscreen view.");
  const window = (await WebviewWindow.getByLabel(VIEWER_LABEL)) ?? (await createViewer(monitor));
  if (!desired) return;
  await activate(window, monitor);
}

/** Shows the one reusable fullscreen window on Main's display. */
export function enterViewerFullscreen(): Promise<void> {
  desired = true;
  return queue(enter);
}

/** Leaves fullscreen before hiding, so the hidden window is never raised. */
export function exitViewerFullscreen(): Promise<void> {
  desired = false;
  return queue(async () => {
    const window = await WebviewWindow.getByLabel(VIEWER_LABEL).catch((error) => {
      reportWindowCall("viewer window lookup")(error);
      return null;
    });
    if (window === null) return;
    await setFullscreen(false).catch(reportWindowCall("viewer leave fullscreen"));
    await window.hide().catch(reportWindowCall("viewer hide"));
  });
}
