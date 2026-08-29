import { beforeEach, describe, expect, it } from "vitest";
import type { ScanProgress } from "../../src/models/scan";
import type { SectionCounts } from "../../src/models/sections";
import { useSectionsStore } from "../../src/state/sections-store";
import { installScanEventWiring } from "../../src/workflows/scan-events";
import { fireEvent, mockCommand, mockCommands, resetTauriMocks } from "../mocks/tauri";

function counts(imageCount: number): SectionCounts {
  return {
    images: imageCount > 0 ? [{ month: "2026-01", count: imageCount }] : [],
    videos: [],
    others: [],
  };
}

function progress(done: number): ScanProgress {
  return {
    phase: "resolve",
    done,
    total: 10,
    currentPath: null,
    discovered: null,
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
    error: null,
    sourceCheck: { running: false, stopping: false, progress: null },
    fileInformation: {
      running: false,
      paused: false,
      stopping: false,
      queued: false,
      progress: null,
    },
    rescanNeeded: false,
  });
});

void installScanEventWiring();

describe("out-of-order count snapshots", () => {
  it("keeps the newer snapshot when the older response arrives last", async () => {
    const resolvers: ((value: SectionCounts) => void)[] = [];
    mockCommand(
      "get_section_counts",
      () => new Promise<SectionCounts>((resolve) => resolvers.push(resolve)),
    );

    const older = useSectionsStore.getState().loadCounts();
    const newer = useSectionsStore.getState().loadCounts();
    resolvers[1](counts(80));
    await newer;
    resolvers[0](counts(3));
    await older;

    expect(useSectionsStore.getState().counts?.images[0]?.count).toBe(80);
  });
});

describe("independent index work", () => {
  it("keeps a failed background command visible", async () => {
    mockCommands({ start_source_check: () => Promise.reject(new Error("busy")) });

    await useSectionsStore.getState().startSourceCheck();

    expect(useSectionsStore.getState().error).toBe("Couldn’t start checking source folders.");
  });

  it("coalesces rapid file-information progress into one refresh", async () => {
    let loads = 0;
    mockCommands({
      get_section_counts: () => {
        loads += 1;
        return counts(loads);
      },
      get_section_items: () => [],
      get_issues: () => ({ total: 0, rows: [] }),
    });

    fireEvent("file-information://progress", progress(1));
    fireEvent("file-information://progress", progress(2));
    fireEvent("file-information://progress", progress(3));
    await new Promise((resolve) => setTimeout(resolve, 275));

    expect(loads).toBe(1);
  });

  it("marks only the source check as stopping after backend acceptance", async () => {
    mockCommands({ stop_source_check: () => true });
    useSectionsStore.setState({
      sourceCheck: { running: true, stopping: false, progress: null },
    });

    await useSectionsStore.getState().stopSourceCheck();

    expect(useSectionsStore.getState().sourceCheck.stopping).toBe(true);
    expect(useSectionsStore.getState().fileInformation.stopping).toBe(false);
  });
});
