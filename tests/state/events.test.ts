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
    // An unknown phase token degrades to a capitalized word, dash-joined —
    // the store speaks user language, never raw pipeline tokens.
    expect(sections.getState().progress).toBe("Hashing \u2014 12/40");
  });

  it("clears the scan on a clean finish", async () => {
    sections.setState({ scanning: true, progress: "Hashing \u2014 1/2" });

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
    sections.setState({ scanning: true, progress: "Hashing \u2014 1/2" });

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
