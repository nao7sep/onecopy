import { invoke } from "@tauri-apps/api/core";
import {
  PhysicalPosition,
  PhysicalSize,
  availableMonitors,
  currentMonitor,
} from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { focusWhileActive, reportWindowCall } from "../repositories";
import { waitForWindowCreated } from "../utils/windowCreation";
import { documentTranslator } from "../i18n/I18nContext";

const WINDOW_LABEL = "fullscreen-view";

interface Display {
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
  return invoke<void>("set_window_fullscreen", { label: WINDOW_LABEL, enable });
}

async function resolveMonitor(): Promise<Display | null> {
  const hosting = await currentMonitor();
  if (hosting !== null) return hosting;
  return (await availableMonitors())[0] ?? null;
}

/** Placed on the display first, so fullscreen takes that display's frame. */
async function activate(window: WebviewWindow, monitor: Display): Promise<void> {
  await window.setPosition(new PhysicalPosition(monitor.position.x, monitor.position.y));
  await window.setSize(new PhysicalSize(monitor.size.width, monitor.size.height));
  await window.show();
  await setFullscreen(true);
  await focusWhileActive(WINDOW_LABEL);
}

async function createWindow(monitor: Display): Promise<WebviewWindow> {
  const scale = monitor.scaleFactor || 1;
  const window = new WebviewWindow(WINDOW_LABEL, {
    url: "index.html?view=fullscreen-view",
    // window-appearance.ts sets the definitive title on first paint; this is
    // only what shows for the brief instant before that (L6).
    title: documentTranslator().t("window.titleFullscreenView"),
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
  await waitForWindowCreated(window, "Fullscreen view");
  return window;
}

async function enter(): Promise<void> {
  const monitor = await resolveMonitor();
  if (monitor === null) throw new Error("No display is available for fullscreen view.");
  const window = (await WebviewWindow.getByLabel(WINDOW_LABEL)) ?? (await createWindow(monitor));
  if (!desired) return;
  await activate(window, monitor);
}

/** Shows the one reusable fullscreen window on Main's display. */
export function showFullscreenViewWindow(): Promise<void> {
  desired = true;
  return queue(enter);
}

/** Leaves fullscreen before hiding, so the hidden window is never raised. */
export function hideFullscreenViewWindow(): Promise<void> {
  desired = false;
  return queue(async () => {
    const window = await WebviewWindow.getByLabel(WINDOW_LABEL).catch((error) => {
      reportWindowCall("fullscreen view window lookup")(error);
      return null;
    });
    if (window === null) return;
    await setFullscreen(false).catch(reportWindowCall("fullscreen view leave fullscreen"));
    await window.hide().catch(reportWindowCall("fullscreen view hide"));
  });
}
