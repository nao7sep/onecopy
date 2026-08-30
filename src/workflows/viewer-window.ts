import { invoke } from "@tauri-apps/api/core";
import {
  PhysicalPosition,
  PhysicalSize,
  availableMonitors,
  currentMonitor,
} from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { reportWindowCall } from "../repositories";

const VIEWER_LABEL = "viewer";

export interface ViewerMonitor {
  position: { x: number; y: number };
  size: { width: number; height: number };
  scaleFactor: number;
}

let desired = false;
let lifecycle: Promise<void> = Promise.resolve();
let fullscreenTransition: Promise<void> = Promise.resolve();

function queue(action: () => Promise<void>): Promise<void> {
  const next = lifecycle.then(action, action);
  lifecycle = next.catch(() => undefined);
  return next;
}

function setSimpleFullscreen(enable: boolean): Promise<void> {
  const next = fullscreenTransition
    .catch(() => undefined)
    .then(() => invoke<void>("set_window_simple_fullscreen", { label: VIEWER_LABEL, enable }));
  fullscreenTransition = next;
  return next;
}

async function resolveMonitor(preferred?: ViewerMonitor): Promise<ViewerMonitor | null> {
  if (preferred !== undefined) return preferred;
  const hosting = await currentMonitor();
  if (hosting !== null) return hosting;
  return (await availableMonitors())[0] ?? null;
}

async function activate(window: WebviewWindow, monitor: ViewerMonitor): Promise<void> {
  await window.setPosition(new PhysicalPosition(monitor.position.x, monitor.position.y));
  await window.setSize(new PhysicalSize(monitor.size.width, monitor.size.height));
  await window.setAlwaysOnTop(true);
  await setSimpleFullscreen(true);
  await window.show();
  await window.setFocus();
}

async function createViewer(monitor: ViewerMonitor): Promise<WebviewWindow> {
  const scale = monitor.scaleFactor || 1;
  const window = new WebviewWindow(VIEWER_LABEL, {
    url: "index.html?view=viewer",
    title: "OneCopy Viewer",
    x: monitor.position.x / scale,
    y: monitor.position.y / scale,
    width: monitor.size.width / scale,
    height: monitor.size.height / scale,
    decorations: false,
    alwaysOnTop: true,
    skipTaskbar: true,
    resizable: false,
    focus: false,
    visible: false,
  });
  await new Promise<void>((resolve, reject) => {
    void window.once("tauri://created", () => resolve()).catch(reject);
    void window.once("tauri://error", (event) => reject(event.payload)).catch(reject);
  });
  return window;
}

async function enter(preferred?: ViewerMonitor): Promise<void> {
  const monitor = await resolveMonitor(preferred);
  if (monitor === null) throw new Error("No display is available for fullscreen view.");
  const window = (await WebviewWindow.getByLabel(VIEWER_LABEL)) ?? (await createViewer(monitor));
  if (!desired) return;
  await activate(window, monitor);
}

/** Shows or repositions the one reusable non-Spaces fullscreen surface. */
export function enterViewerFullscreen(preferred?: ViewerMonitor): Promise<void> {
  desired = true;
  return queue(() => enter(preferred));
}

/** Leaves native presentation before clearing topmost state and hiding. */
export function exitViewerFullscreen(): Promise<void> {
  desired = false;
  return queue(async () => {
    const window = await WebviewWindow.getByLabel(VIEWER_LABEL).catch((error) => {
      reportWindowCall("viewer window lookup")(error);
      return null;
    });
    if (window === null) return;
    await setSimpleFullscreen(false).catch(reportWindowCall("viewer leave fullscreen"));
    await window.setAlwaysOnTop(false).catch(reportWindowCall("viewer clear always on top"));
    await window.hide().catch(reportWindowCall("viewer hide"));
  });
}
