// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EventInstallation } from "../../src/utils/eventInstallation";
import { useComparisonStore } from "../../src/state/comparison-store";
import { useItemsStore } from "../../src/state/items-store";
import {
  closeComparisonImage, installComparisonImageEvents, openComparisonImage,
} from "../../src/workflows/comparison-image";
import {
  createdWindows, emitCalls, fireEvent, invokeCalls, resetTauriMocks,
  setCurrentMonitor, setFocus, setWindowCreatedHook, WebviewWindow,
} from "../mocks/tauri";

const MONITOR = {
  name: "Fixture display", position: { x: 1920, y: 0 }, size: { width: 1920, height: 1080 }, scaleFactor: 2,
};
const MEMBERS = [0, 1].map((index) => ({
  hash: `h${index}`, fileName: `image-${index}.jpg`, width: 4000, height: 3000,
  byteSize: 100, sharpness: null, faceScore: null, copyCount: 1, hasThumb: true,
}));
let listeners: EventInstallation;

beforeEach(async () => {
  resetTauriMocks();
  setCurrentMonitor(MONITOR);
  useComparisonStore.setState({
    open: true, sessionId: 1, members: MEMBERS, page: 0, maximumImages: 4,
    displayCount: 1, busy: false, pendingAction: null, anchor: "h1", selected: new Set(["h0"]),
  });
  useItemsStore.setState({ selectedItem: "main-anchor", selectedKeys: new Set(["main-anchor"]) });
  document.body.innerHTML = '<div id="comparison-item-area" tabindex="0"></div>';
  listeners = new EventInstallation();
  await installComparisonImageEvents(listeners);
  setWindowCreatedHook((label) => {
    queueMicrotask(async () => {
      const window = await WebviewWindow.getByLabel(label);
      const registrations = window?.once.mock.calls as unknown as Array<[string, (event: { payload: unknown }) => void]>;
      registrations.find(([name]) => name === "tauri://created")?.[1]({ payload: {} });
    });
  });
});

afterEach(async () => {
  await closeComparisonImage(undefined, false);
  listeners.rollback();
  setWindowCreatedHook(null);
  document.body.innerHTML = "";
});

describe("Comparison-owned image window", () => {
  it("returns to the same decisions and command area without creating a Main sequence", async () => {
    await openComparisonImage();
    expect(createdWindows).toEqual([expect.objectContaining({
      label: "comparison-image", options: expect.objectContaining({ parent: "main", visible: false }),
    })]);
    const image = emitCalls.find((call) => call.event === "comparison-image://state")?.payload as { token: number; member: { hash: string } };
    expect(image.member.hash).toBe("h1");
    expect(invokeCalls.some((call) => call.command === "viewer_sequence_start")).toBe(false);

    await fireEvent("comparison-image://close", { token: image.token });
    await vi.waitFor(() => expect(document.activeElement?.id).toBe("comparison-item-area"));
    expect(setFocus).toHaveBeenCalled();
    expect(useComparisonStore.getState()).toMatchObject({ page: 0, anchor: "h1", selected: new Set(["h0"]) });
    expect(useItemsStore.getState().selectedItem).toBe("main-anchor");
  });

  it("uses the invoking auxiliary display and returns focus there", async () => {
    const comparison = new WebviewWindow("comparison-1");
    await openComparisonImage("comparison-1", MONITOR);
    expect(createdWindows.at(-1)?.options).toMatchObject({ parent: "comparison-1", x: 1008, width: 864 });
    await closeComparisonImage();
    expect(comparison.setFocus).toHaveBeenCalledOnce();
    expect(emitCalls).toContainEqual({ event: "comparison://focus", payload: { label: "comparison-1" } });
  });

  it("does nothing without a picked card", async () => {
    useComparisonStore.setState({ anchor: null });
    await openComparisonImage();
    expect(createdWindows).toHaveLength(0);
  });

  it("keeps failed close retryable without losing the inspection owner", async () => {
    await openComparisonImage();
    const window = await WebviewWindow.getByLabel("comparison-image");
    window?.destroy.mockRejectedValueOnce(new Error("Fixture native close failure"));
    await closeComparisonImage();
    expect(useComparisonStore.getState().message).toContain("decisions are unchanged");
    await closeComparisonImage();
    expect(window?.destroy).toHaveBeenCalledTimes(2);
    expect(await WebviewWindow.getByLabel("comparison-image")).toBeNull();
  });

  it("invalidates delayed opening when Comparison closes", async () => {
    let release!: () => void;
    setWindowCreatedHook((label) => {
      release = () => { void WebviewWindow.getByLabel(label).then((window) => {
        const registrations = window?.once.mock.calls as unknown as Array<[string, (event: { payload: unknown }) => void]>;
        registrations.find(([name]) => name === "tauri://created")?.[1]({ payload: {} });
      }); };
    });
    const opening = openComparisonImage();
    await vi.waitFor(() => expect(createdWindows).toHaveLength(1));
    const imageWindow = await WebviewWindow.getByLabel("comparison-image");
    useComparisonStore.setState({ open: false });
    release();
    await opening;
    await closeComparisonImage();
    expect(imageWindow?.show).not.toHaveBeenCalled();
    expect(imageWindow?.destroy).toHaveBeenCalledOnce();
  });

  it("closes when the inspected member disappears and ignores obsolete close messages", async () => {
    await openComparisonImage();
    const first = emitCalls.find((call) => call.event === "comparison-image://state")?.payload as { token: number };
    await closeComparisonImage();
    await openComparisonImage();
    const current = await WebviewWindow.getByLabel("comparison-image");
    await fireEvent("comparison-image://close", { token: first.token });
    expect(current?.destroy).not.toHaveBeenCalled();
    useComparisonStore.setState({ members: [MEMBERS[0]] });
    await closeComparisonImage();
    expect(current?.destroy).toHaveBeenCalledOnce();
  });
});
