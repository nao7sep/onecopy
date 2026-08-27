// The sidebar counts under the async read commands.
//
// loadCounts fires from many places at once now — every scan phase
// transition, scan done, watcher updates, manual refreshes — and the command
// no longer serializes FIFO on the main thread. The one rule: the LATEST
// request's snapshot wins, whatever order the responses arrive in.

import { beforeEach, describe, expect, it } from "vitest";
import { useSectionsStore } from "../../src/state/sections-store";
import { installScanEventWiring } from "../../src/workflows/scan-events";
import type { SectionCounts } from "../../src/models/sections";
import type { ScanProgress } from "../../src/models/scan";
import { fireEvent, mockCommand, mockCommands, resetTauriMocks } from "../mocks/tauri";

const drain = () => new Promise((resolve) => setTimeout(resolve, 0));

function counts(imageCount: number): SectionCounts {
  return {
    images: imageCount > 0 ? [{ month: "2026-01", count: imageCount }] : [],
    videos: [],
    others: [],
  };
}

function progress(phase: string, done: number): ScanProgress {
  return {
    phase,
    done,
    total: 10,
    currentPath: null,
    discovered: phase === "walk" ? done : null,
    bytesDone: null,
    bytesTotal: null,
    failures: 0,
    nextPhase: null,
  };
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useSectionsStore.setState({
    counts: null,
    scanning: false,
    stopping: false,
    progress: null,
    rescanNeeded: false,
  });
});

void installScanEventWiring();

describe("out-of-order count snapshots", () => {
  it("keeps the newer snapshot when the older response arrives last", async () => {
    const resolvers: ((c: SectionCounts) => void)[] = [];
    mockCommand(
      "get_section_counts",
      () => new Promise<SectionCounts>((resolve) => resolvers.push(resolve)),
    );

    const older = useSectionsStore.getState().loadCounts();
    const newer = useSectionsStore.getState().loadCounts();
    expect(resolvers).toHaveLength(2);

    resolvers[1](counts(80)); // the newer request answers first...
    await newer;
    resolvers[0](counts(3)); // ...and the stale early-scan snapshot straggles in
    await older;
    await drain();

    // Without the sequence guard the tree rolls back to 3 until something
    // else happens to refresh it — on the 8000-photo scan, "until restart".
    expect(useSectionsStore.getState().counts?.images[0]?.count).toBe(80);
  });
});

describe("scan phase transitions", () => {
  it("reloads counts when the phase changes, not on every progress line", async () => {
    let loads = 0;
    mockCommands({
      get_section_counts: () => {
        loads += 1;
        return counts(loads);
      },
    });

    fireEvent("scan://progress", progress("walk", 1));
    fireEvent("scan://progress", progress("walk", 2));
    fireEvent("scan://progress", progress("walk", 3));
    fireEvent("scan://progress", progress("resolve", 1));
    await drain();

    // One load per PHASE — walk fired three progress lines but loads once;
    // resolve's transition is the one that unsticks "Undated".
    expect(loads).toBe(2);
    fireEvent("scan://done", { cancelled: true });
  });

  it("refreshes durable date checkpoints at a bounded cadence within resolve", async () => {
    let loads = 0;
    mockCommands({
      get_section_counts: () => {
        loads += 1;
        return counts(loads);
      },
    });

    fireEvent("scan://progress", progress("resolve", 1));
    await drain();
    expect(loads).toBe(1);

    fireEvent("scan://progress", progress("resolve", 2));
    fireEvent("scan://progress", progress("resolve", 3));
    fireEvent("scan://progress", progress("resolve", 4));
    await new Promise((resolve) => setTimeout(resolve, 275));

    // Three fast durable rows coalesce to one additional count read.
    expect(loads).toBe(2);
    fireEvent("scan://done", { cancelled: true });
  });

  it("enters stopping only after the backend accepts cancellation", async () => {
    mockCommands({ cancel_scan: () => true });
    useSectionsStore.setState({ scanning: true, stopping: false });

    await useSectionsStore.getState().cancelScan();

    expect(useSectionsStore.getState().stopping).toBe(true);
  });
});
