// The backend event channels.
//
// These listeners are the ONLY paths that clear `scanning` and `installing`,
// so a payload or channel-name drift does not fail loudly — it leaves the
// footer permanently claiming work is in flight. Nothing exercised them.

import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { fireEvent, mockCommands, resetTauriMocks } from "../mocks/tauri";
import { useSectionsStore } from "../../src/state/sections-store";
import { useBinariesStore } from "../../src/state/binaries-store";

// The stores register their event wiring once, in an async IIFE at module
// load, so they are imported at the top of the file and the listener registry
// is deliberately NOT cleared between specs — clearing it would leave every
// store deaf for the rest of the run.
const sections = useSectionsStore;
const binaries = useBinariesStore;

async function settle() {
  for (let i = 0; i < 5; i += 1) await Promise.resolve();
}

beforeAll(settle);

beforeEach(async () => {
  resetTauriMocks({ keepListeners: true });
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
    fireEvent("scan://progress", { phase: "hashing", detail: "12/40" });

    expect(sections.getState().scanning).toBe(true);
    expect(sections.getState().progress).toBe("hashing: 12/40");
  });

  it("clears the scan on a clean finish", async () => {
    sections.setState({ scanning: true, progress: "hashing: 1/2" });

    fireEvent("scan://done", {});
    expect(sections.getState().scanning).toBe(false);
    expect(sections.getState().progress).toBe("");
  });

  it("distinguishes a cancelled finish from a clean one", async () => {
    sections.setState({ scanning: true, rescanNeeded: false });

    fireEvent("scan://done", { cancelled: true });
    expect(sections.getState().scanning).toBe(false);
    // A cancelled walk may have left whole directories unread, so the counts
    // understate the library — it must not read as a clean finish.
    expect(sections.getState().rescanNeeded).toBe(true);
  });

  it("clears the scan on an error rather than leaving the footer stuck", async () => {
    sections.setState({ scanning: true, progress: "hashing: 1/2" });

    fireEvent("scan://error", { message: "index open failed" });
    expect(sections.getState().scanning).toBe(false);
    expect(sections.getState().progress).toBe("");
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

describe("binaries events", () => {
  it("clears the install on an error", async () => {
    binaries.setState({ installingId: "ffmpeg" });

    fireEvent("binaries://error", { id: "ffmpeg", message: "checksum mismatch" });
    expect(binaries.getState().installingId).toBeNull();
  });

  it("tracks WHICH entry is installing from the progress events", async () => {
    // The registry holds several entries now; the modal must narrate the one
    // actually downloading, and the chip must ignore a model's install.
    fireEvent("binaries://progress", {
      id: "whisper-large-v3-turbo",
      phase: "download",
      detail: "300 / 1549 MB",
    });
    expect(binaries.getState().installingId).toBe("whisper-large-v3-turbo");
    expect(binaries.getState().progress).toContain("300 / 1549 MB");
  });
});
