// The multi-monitor comparison handshake.
//
// A secondary comparison window announces itself with `comparison://ready` the
// moment it mounts, and the main window answers by broadcasting the current
// slots. That reply is the ONLY one it gets — broadcasts otherwise fire on
// state changes, and a turn in progress produces none — so an announcement
// that arrives before the session exists leaves that screen showing "Waiting
// for the comparison…" for the whole group.
//
// The ordering is therefore the contract: publish the session, THEN create the
// windows. These specs assert it from both ends.

import { beforeEach, describe, expect, it } from "vitest";
import {
  useComparisonStore,
  perScreenCapacity,
  type GroupMember,
} from "../../src/state/comparison-store";
import { gridColumns } from "../../src/state/comparison-store";
import {
  createdWindows,
  emitCalls,
  mockCommands,
  resetTauriMocks,
  setCurrentMonitor,
  setMonitors,
  setWindowCreatedHook,
} from "../mocks/tauri";

function member(hash: string, width = 4000, height = 3000): GroupMember {
  return {
    hash,
    fileName: `${hash}.jpg`,
    width,
    height,
    byteSize: width * height,
    sharpness: 1,
    copyCount: 1,
    hasThumb: true,
  };
}

const TWO_SCREENS = [
  { name: "one", position: { x: 0, y: 0 }, size: { width: 2560, height: 1440 }, scaleFactor: 2 },
  {
    name: "two",
    position: { x: 2560, y: 0 },
    size: { width: 2560, height: 1440 },
    scaleFactor: 2,
  },
];

beforeEach(() => {
  // Stores register their forwarding listeners once at module load.
  resetTauriMocks({ keepListeners: true });
  useComparisonStore.setState({
    open: false,
    slots: [],
    queue: [],
    kept: new Set<string>(),
    spreadCount: 0,
    busy: false,
  });
});

describe("opening a group across screens", () => {
  it("publishes the session before any window can ask for it", async () => {
    setMonitors(TWO_SCREENS);
    const members = [member("a"), member("b"), member("c")];
    mockCommands({ get_similar_group: () => members, patch_state: () => ({}) });

    // Observed INSIDE the constructor: a real webview begins booting there and
    // can announce itself at once. Reading the store after openGroup returns
    // would pass either way and prove nothing.
    let openAtWindowCreation: boolean | null = null;
    let slotsAtWindowCreation = -1;
    setWindowCreatedHook(() => {
      const state = useComparisonStore.getState();
      openAtWindowCreation = state.open;
      slotsAtWindowCreation = state.slots.length;
    });
    const originalLength = createdWindows.length;

    await useComparisonStore.getState().openGroup("a");

    // A window WAS created (two monitors, so one secondary).
    expect(createdWindows.length).toBeGreaterThan(originalLength);
    // The session must already be live AND populated at that instant.
    expect(openAtWindowCreation).toBe(true);
    expect(slotsAtWindowCreation).toBe(3);

    // And the answer to a late announcement is a real broadcast, not silence.
    const broadcasts = emitCalls.filter((c) => c.event === "comparison://state");
    expect(broadcasts.length).toBeGreaterThan(0);
    const last = broadcasts[broadcasts.length - 1].payload as {
      chunks: unknown[][];
    };
    expect(last.chunks.flat().length).toBe(3);
  });

  it("sizes each window to its own monitor, converted out of physical pixels", async () => {
    setMonitors(TWO_SCREENS);
    mockCommands({
      get_similar_group: () => [member("a"), member("b")],
      patch_state: () => ({}),
    });

    await useComparisonStore.getState().openGroup("a");

    const spread = createdWindows.find((w) => w.label === "comparison-1");
    expect(spread).toBeDefined();
    // The second monitor is 2560x1440 at scale 2, so the LOGICAL window is
    // half that. Passing the physical numbers straight through would open a
    // window twice the screen's size on any Retina display.
    expect(spread?.options.width).toBe(1280);
    expect(spread?.options.height).toBe(720);
    expect(spread?.options.x).toBe(1280);
    // Borderless and above the others — never the OS fullscreen call, whose
    // Space animation costs about a second at both ends of every group.
    expect(spread?.options.decorations).toBe(false);
    expect(spread?.options.alwaysOnTop).toBe(true);
    expect(spread?.options.fullscreen).toBeUndefined();
  });

  it("reuses a hidden window instead of booting a second webview", async () => {
    setMonitors(TWO_SCREENS);
    mockCommands({
      get_similar_group: () => [member("a"), member("b")],
      patch_state: () => ({}),
      get_section_counts: () => ({ images: [], videos: [], others: [] }),
      get_section_items: () => [],
      patch_config: () => ({}),
    });

    await useComparisonStore.getState().openGroup("a");
    const afterFirst = createdWindows.length;

    useComparisonStore.getState().close();
    // Let the hide settle before reopening.
    await new Promise((resolve) => setTimeout(resolve, 0));
    await useComparisonStore.getState().openGroup("a");

    // The window survived the close, so the second session constructs none.
    expect(createdWindows.length).toBe(afterFirst);
  });

  it("stays single-window on one monitor and keeps all sixteen keys", async () => {
    setMonitors([TWO_SCREENS[0]]);
    mockCommands({
      get_similar_group: () => [member("a"), member("b")],
      patch_state: () => ({}),
    });

    await useComparisonStore.getState().openGroup("a");

    expect(createdWindows.filter((w) => w.label.startsWith("comparison-"))).toHaveLength(0);
    expect(useComparisonStore.getState().capacities).toEqual([16]);
  });
});

describe("the spread avoids the main window's own screen", () => {
  it("never covers the main window, wherever it actually is", async () => {
    // The developer's report: "left bottom is my primary screen but I have
    // never seen slots 1-4". The spread targeted priority slots 2+ BLIND, so
    // when the priority list disagreed with where the main window really was,
    // an always-on-top borderless window landed on top of it and buried its
    // chunk. The spread must ask which monitor hosts the main window and
    // skip exactly that one.
    const THREE = [
      ...TWO_SCREENS,
      {
        name: "three",
        position: { x: 5120, y: 0 },
        size: { width: 2560, height: 1440 },
        scaleFactor: 2,
      },
    ];
    setMonitors(THREE);
    // The main window actually lives on the SECOND monitor of the list.
    setCurrentMonitor(THREE[1]);
    mockCommands({
      get_similar_group: () => [member("a"), member("b"), member("c")],
      patch_state: () => ({}),
    });

    await useComparisonStore.getState().openGroup("a");

    const targets = createdWindows
      .filter((w) => w.label.startsWith("comparison-"))
      .map((w) => w.options.x);
    // Windows on monitors 0 (x=0) and 2 (x=5120/2 logical) — never on the
    // hosting monitor (x=2560/2 logical = 1280).
    expect(targets).toHaveLength(2);
    expect(targets).not.toContain(1280);
    expect(targets).toContain(0);
    expect(targets).toContain(2560);
  });
});

describe("slot grids track the photos' shape", () => {
  it("puts four landscape photos in a 2×2 on a landscape screen", () => {
    expect(gridColumns(4, 16 / 9, false)).toBe(2);
  });

  it("stands three portrait photos abreast on a landscape screen", () => {
    expect(gridColumns(3, 16 / 9, true)).toBe(3);
  });

  it("stacks landscape photos on a portrait screen", () => {
    expect(gridColumns(4, 9 / 16, false)).toBe(1);
  });

  it("one photo takes the whole window", () => {
    expect(gridColumns(1, 16 / 9, false)).toBe(1);
  });
});

describe("per-screen capacity", () => {
  it("follows the group's dominant orientation, not the monitor's", () => {
    expect(perScreenCapacity([member("a", 3000, 4000), member("b", 3000, 4000)])).toBe(3);
    expect(perScreenCapacity([member("a"), member("b")])).toBe(4);
  });
});
