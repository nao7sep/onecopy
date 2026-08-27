// The backend event channels.
//
// These listeners are the ONLY paths that clear `scanning` and `installing`,
// so a payload or channel-name drift does not fail loudly — it leaves the
// footer permanently claiming work is in flight. Nothing exercised them.

import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { fireEvent, invokeCalls, listenerCount, mockCommands, resetTauriMocks } from "../mocks/tauri";
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
  sections.setState({ scanning: false, stopping: false, progress: null, rescanNeeded: false });
  mockCommands({
    get_section_counts: () => [],
    get_section_items: () => [],
    get_issues: () => ({ total: 0, rows: [] }),
    patch_state: () => ({}),
    binaries_state: () => ({ status: "up-to-date" }),
  });
  await settle();
});

describe("scan events", () => {
  it("reports progress and marks the scan running", async () => {
    const progress = scanProgress({ phase: "hashing", done: 12, total: 40 });
    fireEvent("scan://progress", progress);

    expect(sections.getState().scanning).toBe(true);
    // Event wiring projects typed backend facts; presentation owns the words.
    expect(sections.getState().progress).toEqual(progress);
  });

  it("clears the scan on a clean finish", async () => {
    const indexed = scanProgress({
      phase: "indexed",
      done: 1,
      total: 1,
      currentPath: null,
      nextPhase: null,
    });
    fireEvent("scan://progress", indexed);

    fireEvent("scan://done", {});
    expect(sections.getState().scanning).toBe(false);
    expect(sections.getState().progress).toEqual(indexed);
  });

  it("distinguishes a cancelled finish from a clean one", async () => {
    sections.setState({ scanning: true, rescanNeeded: false });

    fireEvent("scan://done", { cancelled: true });
    expect(sections.getState().scanning).toBe(false);
    expect(sections.getState().stopping).toBe(false);
    // A cancelled walk may have left whole directories unread, so the counts
    // understate the library — it must not read as a clean finish.
    expect(sections.getState().rescanNeeded).toBe(true);
  });

  it("clears the scan on an error rather than leaving the footer stuck", async () => {
    sections.setState({ scanning: true, progress: scanProgress() });

    fireEvent("scan://error", { message: "index open failed" });
    expect(sections.getState().scanning).toBe(false);
    expect(sections.getState().stopping).toBe(false);
    expect(sections.getState().progress).toBeNull();
  });
});

describe("watcher events", () => {
  it("flags rescan-needed on overflow", async () => {
    sections.setState({ rescanNeeded: false });

    fireEvent("watch://rescan-needed", {});
    expect(sections.getState().rescanNeeded).toBe(true);
  });

  it("clears the flag when a scan STARTS, not when one finishes", async () => {
    // The repair is the scan itself, so the flag drops as the walk begins.
    // A finish event must not clear it — a cancelled run reaches `done` too,
    // and clearing there would hide a half-indexed library.
    sections.setState({ rescanNeeded: true, scanning: false });
    mockCommands({ start_scan: () => true });

    await sections.getState().startScan();
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
          namesDiffer: false,
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
    expect(invokeCalls.some((call) => call.command === "get_section_items")).toBe(false);
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
    binaries.setState({ installing: { ffmpeg: "Downloading — 12 MB" } });

    fireEvent("binaries://error", { id: "ffmpeg", message: "checksum mismatch" });
    expect(binaries.getState().installing["ffmpeg"]).toBeUndefined();
    expect(binaries.getState().errors["ffmpeg"]).toBe("checksum mismatch");
  });

  it("clears progress without inventing an error when an install is cancelled", async () => {
    binaries.setState({
      installing: { ffmpeg: "Cancelling…" },
      errors: { ffmpeg: "old failure" },
    });

    fireEvent("binaries://cancelled", { id: "ffmpeg" });
    expect(binaries.getState().installing.ffmpeg).toBeUndefined();
    expect(binaries.getState().errors.ffmpeg).toBeUndefined();
  });

  it("narrates SEVERAL installs at once, each in words", async () => {
    // Installs are parallel per entry (developer, 2026-08-17): the map keeps
    // one humanized line per id — "download" never reaches the user raw.
    fireEvent("binaries://progress", {
      id: "whisper-large-v3-turbo",
      phase: "download",
      detail: "300 / 1549 MB",
    });
    fireEvent("binaries://progress", {
      id: "ultraface-rfb640",
      phase: "verify",
      detail: "checking integrity",
    });
    const installing = binaries.getState().installing;
    expect(installing["whisper-large-v3-turbo"]).toBe("Downloading — 300 / 1549 MB");
    expect(installing["ultraface-rfb640"]).toBe("Verifying — checking integrity");
  });

  it("retains each phase and a terminal result instead of flashing one line", async () => {
    binaries.setState({ installing: {}, installHistory: {} });
    fireEvent("binaries://progress", {
      id: "ffmpeg",
      phase: "resolve",
      detail: "finding the latest build",
    });
    fireEvent("binaries://progress", {
      id: "ffmpeg",
      phase: "download",
      detail: "84 MB",
    });
    fireEvent("binaries://progress", {
      id: "ffmpeg",
      phase: "verify",
      detail: "checking integrity",
    });
    fireEvent("binaries://done", { id: "ffmpeg" });

    expect(binaries.getState().installHistory.ffmpeg).toEqual([
      { phase: "resolve", text: "Resolving — finding the latest build" },
      { phase: "download", text: "Downloading — 84 MB" },
      { phase: "verify", text: "Verifying — checking integrity" },
      { phase: "result", text: "Installed" },
    ]);
  });
});
