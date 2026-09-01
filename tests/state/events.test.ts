// The backend event channels.
//
// These listeners are the ONLY paths that clear library work and installs,
// so a payload or channel-name drift does not fail loudly — it leaves the
// footer permanently claiming work is in flight. Nothing exercised them.

import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import {
  fireEvent,
  invokeCalls,
  listenerCount,
  mockCommands,
  mockSectionItems,
  resetTauriMocks,
} from "../mocks/tauri";
import { useSectionsStore } from "../../src/state/sections-store";
import { useBinariesStore } from "../../src/state/binaries-store";
import { installScanEventWiring } from "../../src/workflows/scan-events";
import type { ScanProgress } from "../../src/models/scan";
import { EMPTY_ITEM_WORK } from "../../src/models/items";

// The application workflow registers event wiring once. The listener registry
// is deliberately NOT cleared between specs — clearing it would leave this
// module's idempotent installation deaf for the rest of the run.
const sections = useSectionsStore;
const binaries = useBinariesStore;

function scanProgress(overrides: Partial<ScanProgress> = {}): ScanProgress {
  return {
    phase: "hash",
    done: 1,
    total: 2,
    currentPath: "/photos/a.jpg",
    discovered: null,
    bytesDone: null,
    bytesTotal: null,
    failures: 0,
    nextPhase: "extract",
    ...overrides,
  };
}

void installScanEventWiring();

async function settle() {
  for (let i = 0; i < 5; i += 1) await Promise.resolve();
}

async function settleUntil(predicate: () => boolean) {
  for (let i = 0; i < 50 && !predicate(); i += 1) await Promise.resolve();
}

beforeAll(async () => {
  for (let i = 0; i < 20 && listenerCount("derived://item") === 0; i += 1) {
    await Promise.resolve();
  }
  expect(listenerCount("derived://item")).toBe(1);
});

beforeEach(async () => {
  resetTauriMocks({ keepListeners: true });
  sections.setState({
    sourceCheck: {
      running: false,
      stopping: false,
      lastResult: "stopped",
      eventSequence: 0,
      progress: null,
    },
    fileInformation: {
      running: false,
      paused: false,
      stopping: false,
      queued: false,
      eventSequence: 0,
      progress: null,
    },
    rescanNeeded: false,
  });
  mockCommands({
    get_section_counts: () => [],
    get_issues: () => ({ total: 0, rows: [] }),
    index_work_snapshot: () => ({
      sourceCheck: {
        running: false,
        stopping: false,
        lastResult: "stopped",
        eventSequence: 0,
      },
      fileInformation: {
        running: false,
        paused: false,
        stopping: false,
        queued: false,
        eventSequence: 0,
      },
    }),
    patch_state: () => ({}),
    binaries_state: () => ({ status: "up-to-date" }),
  });
  mockSectionItems(() => []);
  await settle();
});

describe("library work events", () => {
  it("reports file-information progress independently", async () => {
    const progress = scanProgress({ phase: "hashing", done: 12, total: 40 });
    fireEvent("file-information://progress", { eventSequence: 1, progress });

    expect(sections.getState().fileInformation.running).toBe(true);
    expect(sections.getState().sourceCheck.running).toBe(false);
    expect(sections.getState().fileInformation.progress).toEqual(progress);
  });

  it("clears file-information work on a clean finish", async () => {
    fireEvent("file-information://progress", {
      eventSequence: 1,
      progress: scanProgress(),
    });
    fireEvent("file-information://done", { eventSequence: 2 });
    expect(sections.getState().fileInformation.running).toBe(false);
    expect(sections.getState().fileInformation.progress).toBeNull();
  });

  it("marks a stopped source check as still needing reconciliation", async () => {
    sections.setState({
      sourceCheck: {
        running: true,
        stopping: true,
        lastResult: "stopped",
        eventSequence: 4,
        progress: scanProgress(),
      },
      rescanNeeded: false,
    });

    fireEvent("source-check://done", { eventSequence: 5, stopped: true });
    expect(sections.getState().sourceCheck.running).toBe(false);
    expect(sections.getState().sourceCheck.stopping).toBe(false);
    expect(sections.getState().sourceCheck.lastResult).toBe("stopped");
    expect(sections.getState().rescanNeeded).toBe(true);
  });

  it("clears a failed source check rather than leaving the footer stuck", async () => {
    sections.setState({
      sourceCheck: {
        running: true,
        stopping: false,
        lastResult: "stopped",
        eventSequence: 7,
        progress: scanProgress(),
      },
    });

    fireEvent("source-check://done", { eventSequence: 8, error: "index open failed" });
    expect(sections.getState().sourceCheck.running).toBe(false);
    expect(sections.getState().sourceCheck.progress).toBeNull();
    expect(sections.getState().sourceCheck.lastResult).toBe("failed");
  });

  it("retains a clean source check as completed", () => {
    fireEvent("source-check://done", { eventSequence: 1 });
    expect(sections.getState().sourceCheck.lastResult).toBe("completed");
  });

  it("does not let delayed progress or state resurrect settled work", () => {
    const progress = scanProgress();
    fireEvent("source-check://progress", { eventSequence: 2, progress });
    fireEvent("source-check://done", { eventSequence: 4 });
    fireEvent("source-check://progress", { eventSequence: 3, progress });
    fireEvent("source-check://state", {
      running: true,
      stopping: true,
      lastResult: "stopped",
      eventSequence: 1,
    });

    expect(sections.getState().sourceCheck.running).toBe(false);
    expect(sections.getState().sourceCheck.stopping).toBe(false);
    expect(sections.getState().sourceCheck.lastResult).toBe("completed");
  });
});

describe("watcher events", () => {
  it("flags rescan-needed on overflow", async () => {
    sections.setState({ rescanNeeded: false });

    fireEvent("watch://rescan-needed", {});
    expect(sections.getState().rescanNeeded).toBe(true);
  });

  it("clears the flag when a source-folder check starts", async () => {
    sections.setState({ rescanNeeded: true });
    mockCommands({ start_source_check: () => true });

    await sections.getState().startSourceCheck();
    expect(sections.getState().rescanNeeded).toBe(false);
  });
});

describe("derived media events", () => {
  it("patches one item without re-reading the open section or its counts", async () => {
    const { useItemsStore } = await import("../../src/state/items-store");
    useItemsStore.setState({
      selected: { kind: "image", month: "2026-01" },
      items: [
        {
          hash: "h1",
          pathId: 1,
          fileName: "one.jpg",
          resolvedUtcMs: 1,
          copyCount: 1,
          width: null,
          height: null,
          hasThumb: false,
          similarGroupId: null,
          sharpness: null,
          faceScore: null,
          byteSize: 1,
          hasCompanions: false,
          durationMs: null,
          dirPaths: ["/photos"],
          derivedWork: EMPTY_ITEM_WORK,
        },
      ],
    });

    fireEvent("derived://item", {
      previousHash: "h1",
      item: { ...useItemsStore.getState().items[0], width: 4000, hasThumb: true },
    });
    await settleUntil(() => useItemsStore.getState().items[0]?.width === 4000);

    expect(useItemsStore.getState().items[0]).toMatchObject({ width: 4000, hasThumb: true });
    expect(invokeCalls.some((call) => call.command === "get_section_window")).toBe(false);
    expect(invokeCalls.some((call) => call.command === "reconcile_section")).toBe(false);
    expect(invokeCalls.some((call) => call.command === "get_section_counts")).toBe(false);
  });
});

describe("quarantine events", () => {
  it("carries a mid-session quarantine to the reporting surface", async () => {
    // A patch reads the file it is about to merge into, so a store can be set
    // aside long after boot — where no load result exists to carry the record
    // home. It arrives as an event instead, into the same list.
    const { useAppStore } = await import("../../src/state/app-store");
    useAppStore.setState({ quarantines: [] });

    fireEvent("storage://quarantined", {
      quarantines: [{ file: "config.json", quarantinedTo: "/root/config-x.invalid" }],
    });

    expect(useAppStore.getState().quarantines).toEqual([
      { file: "config.json", quarantinedTo: "/root/config-x.invalid" },
    ]);
  });
});

describe("binaries events", () => {
  it("clears the entry and keeps the failure visible on an error", async () => {
    binaries.setState({
      installing: {
        ffmpeg: {
          progress: { phase: "download", done: 12, total: null, nextPhase: "verify" },
          cancelling: false,
        },
      },
    });

    fireEvent("binaries://error", { id: "ffmpeg", message: "checksum mismatch" });
    expect(binaries.getState().installing["ffmpeg"]).toBeUndefined();
    expect(binaries.getState().errors["ffmpeg"]).toBe("checksum mismatch");
  });

  it("clears progress without inventing an error when an install is cancelled", async () => {
    binaries.setState({
      installing: { ffmpeg: { progress: null, cancelling: true } },
      errors: { ffmpeg: "old failure" },
    });

    fireEvent("binaries://cancelled", { id: "ffmpeg" });
    expect(binaries.getState().installing.ffmpeg).toBeUndefined();
    expect(binaries.getState().errors.ffmpeg).toBeUndefined();
  });

  it("narrates SEVERAL installs at once, each in words", async () => {
    // Installs are parallel per entry (developer, 2026-08-17): the map keeps
    // one typed snapshot per id; presentation owns the humanized line.
    fireEvent("binaries://progress", {
      id: "whisper-large-v3-turbo",
      phase: "download",
      done: 300 * 1_048_576,
      total: 1_549 * 1_048_576,
      nextPhase: "verify",
    });
    fireEvent("binaries://progress", {
      id: "ultraface-rfb640",
      phase: "verify",
      done: 0,
      total: 1_588_012,
      nextPhase: "install",
    });
    const installing = binaries.getState().installing;
    expect(installing["whisper-large-v3-turbo"]?.progress).toEqual({
      phase: "download",
      done: 300 * 1_048_576,
      total: 1_549 * 1_048_576,
      nextPhase: "verify",
    });
    expect(installing["ultraface-rfb640"]?.progress).toEqual({
      phase: "verify",
      done: 0,
      total: 1_588_012,
      nextPhase: "install",
    });
  });

  it("retains each phase and a terminal result instead of flashing one line", async () => {
    binaries.setState({ installing: {}, installHistory: {} });
    fireEvent("binaries://progress", {
      id: "ffmpeg",
      phase: "resolve",
      done: 1,
      total: 1,
      nextPhase: "download",
    });
    fireEvent("binaries://progress", {
      id: "ffmpeg",
      phase: "download",
      done: 84 * 1_048_576,
      total: 84 * 1_048_576,
      nextPhase: "verify",
    });
    fireEvent("binaries://progress", {
      id: "ffmpeg",
      phase: "verify",
      done: 84 * 1_048_576,
      total: 84 * 1_048_576,
      nextPhase: "install",
    });
    fireEvent("binaries://done", { id: "ffmpeg" });

    expect(binaries.getState().installHistory.ffmpeg).toEqual([
      { phase: "resolve", text: "Resolving — 1/1 · Next: Downloading" },
      { phase: "download", text: "Downloading — 84 MB / 84 MB (100%) · Next: Verifying" },
      { phase: "verify", text: "Verifying — 84 MB / 84 MB (100%) · Next: Installing" },
      { phase: "result", text: "Installed" },
    ]);
  });
});
