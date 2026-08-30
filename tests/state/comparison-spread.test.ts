import { beforeEach, describe, expect, it } from "vitest";
import {
  recoverComparisonDisplay,
  useComparisonStore,
  type GroupMember,
} from "../../src/state/comparison-store";
import {
  createdWindows,
  emitCalls,
  invokeCalls,
  mockCommands,
  resetTauriMocks,
  setCurrentMonitor,
  setMonitors,
  setWindowCreatedHook,
  WebviewWindow,
} from "../mocks/tauri";

function member(index: number, portrait = false): GroupMember {
  return {
    hash: `m${index}`,
    fileName: `image-${index}.jpg`,
    width: portrait ? 3000 : 4000,
    height: portrait ? 4000 : 3000,
    byteSize: 1000,
    sharpness: 1,
    faceScore: null,
    copyCount: 1,
    hasThumb: true,
  };
}

function members(count: number, portrait = false): GroupMember[] {
  return Array.from({ length: count }, (_, index) => member(index, portrait));
}

const THREE_SCREENS = [
  {
    name: "one",
    position: { x: 0, y: 0 },
    size: { width: 2560, height: 1440 },
    scaleFactor: 2,
  },
  {
    name: "two",
    position: { x: 2560, y: 0 },
    size: { width: 2560, height: 1440 },
    scaleFactor: 2,
  },
  {
    name: "three",
    position: { x: 5120, y: 0 },
    size: { width: 2560, height: 1440 },
    scaleFactor: 2,
  },
];

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useComparisonStore.setState({
    sessionId: 0,
    open: false,
    members: [],
    originalMemberHashes: [],
    page: 0,
    displayAspects: [16 / 9],
    selected: new Set(),
    anchors: new Set(),
    anchor: null,
    busy: false,
    pendingAction: null,
    failure: null,
    spreadCount: 0,
  });
  mockCommands({ set_window_simple_fullscreen: () => null });
});

describe("opening Comparison across displays", () => {
  it("does not hide Preview for an invalid group", async () => {
    const preview = new WebviewWindow("preview");
    mockCommands({ get_similar_group: () => [member(0)] });
    expect(await useComparisonStore.getState().openGroup("m0")).toBe(
      "unavailable",
    );
    expect(preview.hide).not.toHaveBeenCalled();
  });

  it("publishes the session before a secondary window can announce itself", async () => {
    setMonitors(THREE_SCREENS);
    mockCommands({ get_similar_group: () => members(8) });
    let openAtCreation = false;
    let membersAtCreation = 0;
    setWindowCreatedHook(() => {
      openAtCreation = useComparisonStore.getState().open;
      membersAtCreation = useComparisonStore.getState().members.length;
    });

    await useComparisonStore.getState().openGroup("m0");

    expect(openAtCreation).toBe(true);
    expect(membersAtCreation).toBe(8);
    expect(emitCalls.some((call) => call.event === "comparison://state")).toBe(
      true,
    );
  });

  it("uses only displays needed by the current page", async () => {
    setMonitors(THREE_SCREENS);
    mockCommands({ get_similar_group: () => members(6) });

    await useComparisonStore.getState().openGroup("m0");

    expect(useComparisonStore.getState().capacities).toEqual([4, 4]);
    expect(
      createdWindows.filter((window) => window.label.startsWith("comparison-")),
    ).toHaveLength(1);
  });

  it("uses portrait capacity from the current page", async () => {
    setMonitors(THREE_SCREENS);
    mockCommands({ get_similar_group: () => members(8, true) });

    await useComparisonStore.getState().openGroup("m0");

    expect(useComparisonStore.getState().capacities).toEqual([3, 3, 3]);
    expect(useComparisonStore.getState().portraitDominant).toBe(true);
  });

  it("honors the configured cap even when more displays are available", async () => {
    setMonitors(THREE_SCREENS);
    mockCommands({ get_similar_group: () => members(12) });

    await useComparisonStore.getState().openGroup("m0", ["m0"], "m0", 5);

    expect(useComparisonStore.getState().capacities).toEqual([4, 4]);
    expect(
      (
        emitCalls.filter((call) => call.event === "comparison://state").at(-1)
          ?.payload as {
          chunks: unknown[][];
        }
      ).chunks.flat(),
    ).toHaveLength(5);
  });

  it("avoids the display that hosts the main window", async () => {
    setMonitors(THREE_SCREENS);
    setCurrentMonitor(THREE_SCREENS[1]);
    mockCommands({ get_similar_group: () => members(10) });

    await useComparisonStore.getState().openGroup("m0");

    const targets = createdWindows
      .filter((window) => window.label.startsWith("comparison-"))
      .map((window) => window.options.x);
    expect(targets).toContain(0);
    expect(targets).toContain(2560);
    expect(targets).not.toContain(1280);
  });

  it("sizes a new display window in logical coordinates", async () => {
    setMonitors(THREE_SCREENS.slice(0, 2));
    mockCommands({ get_similar_group: () => members(6) });

    await useComparisonStore.getState().openGroup("m0");

    const spread = createdWindows.find(
      (window) => window.label === "comparison-1",
    );
    expect(spread?.options).toMatchObject({
      x: 1280,
      width: 1280,
      height: 720,
      decorations: false,
      alwaysOnTop: true,
      focus: false,
    });
  });

  it("reuses a hidden secondary window", async () => {
    setMonitors(THREE_SCREENS.slice(0, 2));
    mockCommands({ get_similar_group: () => members(6) });
    await useComparisonStore.getState().openGroup("m0");
    const count = createdWindows.length;
    await useComparisonStore.getState().close();
    await useComparisonStore.getState().openGroup("m0");

    expect(createdWindows).toHaveLength(count);
    expect(
      invokeCalls
        .filter((call) => call.command === "set_window_simple_fullscreen")
        .map((call) => call.args.enable),
    ).toEqual([false, true]);
  });

  it("repaginates on the other surviving displays after one fails", async () => {
    setMonitors(THREE_SCREENS);
    mockCommands({ get_similar_group: () => members(10) });
    await useComparisonStore.getState().openGroup("m0");

    await recoverComparisonDisplay(1);

    expect(useComparisonStore.getState().open).toBe(true);
    expect(useComparisonStore.getState().members).toHaveLength(10);
    expect(useComparisonStore.getState().displayCount).toBe(2);
    const replacement = createdWindows
      .filter((window) => window.label === "comparison-1")
      .at(-1);
    expect(replacement?.options.x).toBe(2560);
  });
});
