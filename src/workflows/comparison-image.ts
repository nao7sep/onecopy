// Comparison owns this single-image inspection window. It never acquires a
// Main viewer sequence or writes library selection/keep decisions.
import { emit } from "@tauri-apps/api/event";
import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { ComparisonMember } from "../models/comparisonSession";
import { useComparisonStore, visibleMembers } from "../state/comparison-store";
import { log, reportWindowCall, toErrorFields } from "../repositories";
import type { EventInstallation } from "../utils/eventInstallation";
import type { MonitorRect } from "../utils/windowBounds";
import { waitForWindowCreated } from "../utils/windowCreation";

const LABEL = "comparison-image";

export interface ComparisonImage {
  token: number;
  sessionId: number;
  member: ComparisonMember;
  returnWindow: string;
}

let active: ComparisonImage | null = null;
let nextToken = 0;
let lifecycle = Promise.resolve();
let closing: Promise<void> | null = null;

function queue(action: () => Promise<void>): Promise<void> {
  const next = lifecycle.then(action, action);
  lifecycle = next.catch(() => undefined);
  return next;
}

function reportFailure(error: unknown): void {
  log.error("comparison image window failed", toErrorFields(error));
  useComparisonStore.setState({ message: "Couldn’t open or close the larger image. Your comparison decisions are unchanged." });
}

export async function focusComparison(returnWindow = "main"): Promise<void> {
  const target = returnWindow === "main" ? getCurrentWindow()
    : await WebviewWindow.getByLabel(returnWindow);
  if (target === null) return focusComparison();
  await target.setFocus();
  if (returnWindow === "main") document.getElementById("comparison-item-area")?.focus();
  else await emit("comparison://focus", { label: returnWindow });
}

export function closeComparisonImage(token?: number, restoreFocus = true): Promise<void> {
  const image = active;
  if (image === null || (token !== undefined && image.token !== token)) return lifecycle;
  if (closing !== null) return closing;
  closing = queue(async () => {
    const window = await WebviewWindow.getByLabel(LABEL);
    await window?.destroy();
    if (active === image) active = null;
    if (restoreFocus && useComparisonStore.getState().open && active === null) {
      await focusComparison(image.returnWindow);
    }
  }).catch(reportFailure).finally(() => { closing = null; });
  return closing;
}

export function openComparisonImage(returnWindow = "main", preferredMonitor?: MonitorRect): Promise<void> {
  if (closing !== null) return closing.then(() => openComparisonImage(returnWindow, preferredMonitor));
  const comparison = useComparisonStore.getState();
  if (!comparison.open || comparison.busy || comparison.pendingAction !== null) return Promise.resolve();
  const member = visibleMembers(comparison).find((item) => item.hash === comparison.anchor);
  if (member === undefined) return Promise.resolve();
  if (active !== null) return queue(async () => {
    await (await WebviewWindow.getByLabel(LABEL))?.setFocus();
  }).catch(reportFailure);
  const image: ComparisonImage = {
    token: ++nextToken, sessionId: comparison.sessionId, member, returnWindow,
  };
  active = image;
  return queue(async () => {
    const monitor = preferredMonitor ?? await currentMonitor();
    if (active !== image || closing !== null) return;
    if (monitor === null) throw new Error("No display is available for Comparison inspection.");
    const area = monitor.workArea ?? monitor;
    const scale = monitor.scaleFactor || 1;
    const window = new WebviewWindow(LABEL, {
      url: "index.html?view=comparison-image", title: member.fileName,
      parent: returnWindow, visible: false, focus: false,
      x: (area.position.x + area.size.width * 0.05) / scale,
      y: (area.position.y + area.size.height * 0.05) / scale,
      width: area.size.width * 0.9 / scale, height: area.size.height * 0.85 / scale,
    });
    await waitForWindowCreated(window, "Comparison image");
    await window.onCloseRequested((event) => {
      event.preventDefault();
      void closeComparisonImage(image.token);
    });
    await window.once("tauri://destroyed", () => {
      if (active === image) {
        active = null;
        if (closing === null && useComparisonStore.getState().open) {
          void focusComparison(image.returnWindow).catch(reportWindowCall("comparison image return focus"));
        }
      }
    });
    if (active !== image || closing !== null) return;
    await emit("comparison-image://state", image);
    await window.show();
    await window.setFocus();
  }).catch(async (error) => {
    if (active === image) await closeComparisonImage(image.token);
    reportFailure(error);
  });
}

export async function installComparisonImageEvents(listeners: EventInstallation): Promise<void> {
  await listeners.listen("comparison-image://ready", () => {
    void emit("comparison-image://state", active).catch(reportWindowCall("comparison image state"));
  });
  await listeners.listen<{ token: number }>("comparison-image://close", (event) => {
    void closeComparisonImage(event.payload.token);
  });
  listeners.retain(useComparisonStore.subscribe((state, previous) => {
    if (state.open === previous.open && state.sessionId === previous.sessionId && state.members === previous.members) return;
    if (active !== null && (!state.open || state.sessionId !== active.sessionId
      || !state.members.some((member) => member.hash === active?.member.hash))) {
      void closeComparisonImage(undefined, state.open);
    }
  }));
}
