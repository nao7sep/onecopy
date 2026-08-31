import { describe, expect, it } from "vitest";
import { libraryLine, statusLine } from "../../src/models/status";
import type { SectionCounts } from "../../src/models/sections";
import type { ScanProgress } from "../../src/models/scan";

const COUNTS: SectionCounts = {
  images: [
    { month: "2016-01", count: 1200 },
    { month: "undated", count: 4 },
  ],
  videos: [{ month: "2016-01", count: 87 }],
  others: [],
};

const IDLE = {
  message: null,
  mutation: null,
  mutationResult: null,
  exiting: false,
  scanning: false,
  stopping: false,
  progress: null,
  rescanNeeded: false,
  counts: COUNTS,
};

const HASH_PROGRESS: ScanProgress = {
  phase: "hash",
  done: 30,
  total: 900,
  currentPath: "/photos/large.mov",
  discovered: null,
  bytesDone: 50,
  bytesTotal: 100,
  failures: 0,
  nextPhase: "extract",
};

describe("the library line", () => {
  it("totals every month of each kind, undated included", () => {
    expect(libraryLine(COUNTS)).toBe("1,204 images · 87 videos");
  });

  it("omits a kind that has nothing rather than printing a zero", () => {
    expect(libraryLine(COUNTS)).not.toContain("other");
  });

  it("says one image, not 1 images", () => {
    expect(
      libraryLine({ images: [{ month: "2016-01", count: 1 }], videos: [], others: [] }),
    ).toBe("1 image");
  });
});

describe("what the status bar shows", () => {
  it("is NEVER blank", () => {
    // The whole finding: the bar rendered scan progress and nothing else, so
    // it stood empty except during a scan and looked broken.
    const cases = [
      IDLE,
      { ...IDLE, counts: null },
      { ...IDLE, counts: { images: [], videos: [], others: [] } },
      { ...IDLE, scanning: true },
      { ...IDLE, rescanNeeded: true },
      { ...IDLE, message: "2 files could not be deleted — see Issues." },
    ];
    for (const input of cases) {
      expect(statusLine(input).text).not.toBe("");
    }
  });

  it("shows the standing library totals when nothing is happening", () => {
    expect(statusLine(IDLE)).toMatchObject({
      tone: "normal",
      text: "1,204 images · 87 videos",
    });
  });

  it("puts a failed delete above everything else", () => {
    // It is the one thing the user just did that did not happen; a scan
    // starting behind it must not bury the news.
    const status = statusLine({
      ...IDLE,
      message: "2 files could not be deleted — see Issues.",
      scanning: true,
      progress: HASH_PROGRESS,
      rescanNeeded: true,
    });
    expect(status.tone).toBe("danger");
    expect(status.text).toContain("could not be deleted");
  });

  it("shows an explicit mutation above background indexing", () => {
    const status = statusLine({
      ...IDLE,
      mutation: {
        cancelling: false,
        progress: {
          operationId: 4,
          kind: "delete",
          phase: "deleting",
          itemsDone: 2,
          itemsTotal: 8,
          filesDone: 3,
          filesTotal: 10,
          bytesDone: 1024,
          bytesTotal: 2048,
          failures: 0,
          currentFileBytesDone: null,
          currentFileBytesTotal: null,
          nextPhase: "complete",
        },
      },
      scanning: true,
      progress: HASH_PROGRESS,
    });
    expect(status.text).toBe("Deleting — 2/8 items · 3/10 files · 1 KB/2 KB");
  });

  it("shows the truthful final operation accounting until it is dismissed", () => {
    const status = statusLine({
      ...IDLE,
      mutationResult: {
        operationId: 9,
        kind: "destination-move",
        cancelled: true,
        summary: {
          itemsCompleted: 2,
          itemsPartial: 1,
          itemsUnstarted: 4,
          filesCompleted: 5,
          filesFailed: 1,
          filesUnstarted: 6,
          error: null,
        },
      },
    });
    expect(status.tone).toBe("warning");
    expect(status.text).toBe(
      "Move cancelled — 2 completed · 1 partially processed · 5 file steps completed · 1 failed · 4 unstarted",
    );
  });

  it("puts the safe close wait above every ordinary status", () => {
    const status = statusLine({
      ...IDLE,
      exiting: true,
      message: "an older message",
    });
    expect(status.text).toBe("Finishing current file before exit…");
  });

  it("prefers live scan progress over the totals it is busy changing", () => {
    const status = statusLine({ ...IDLE, scanning: true, progress: HASH_PROGRESS });
    expect(status.text).toBe("Reading files — 30/900 · large.mov · 50%");
    expect(status.title).toContain("Cloud placeholders");
  });

  it("names file-information work before it reports a phase", () => {
    expect(statusLine({ ...IDLE, scanning: true, progress: null }).text).toBe(
      "Completing file information…",
    );
  });

  it("shows cooperative stopping instead of stale phase progress", () => {
    const status = statusLine({
      ...IDLE,
      scanning: true,
      stopping: true,
      progress: HASH_PROGRESS,
    });
    expect(status.text).toBe("Pausing file-information work…");
    expect(status.title).toContain("current safe step");
  });

  it("keeps an indexed terminal state beside the standing totals", () => {
    const progress: ScanProgress = {
      ...HASH_PROGRESS,
      phase: "indexed",
      done: 1,
      total: 1,
      currentPath: null,
      bytesDone: null,
      bytesTotal: null,
      nextPhase: null,
    };
    expect(statusLine({ ...IDLE, progress })).toMatchObject({
      text: "Up to date · 1,204 images · 87 videos",
      title: expect.stringContaining("Background work"),
    });
  });

  it("warns when indexing finishes with recoverable file failures", () => {
    const progress: ScanProgress = {
      ...HASH_PROGRESS,
      phase: "indexed",
      done: 1,
      total: 1,
      currentPath: null,
      bytesDone: null,
      bytesTotal: null,
      failures: 2,
      nextPhase: null,
    };
    expect(statusLine({ ...IDLE, progress })).toMatchObject({
      tone: "warning",
      text: "Indexed — 2 failed · open Issues · 1,204 images · 87 videos",
    });
  });

  it("warns that the index is knowingly incomplete", () => {
    const status = statusLine({ ...IDLE, rescanNeeded: true });
    expect(status.tone).toBe("warning");
    expect(status.title).toContain("Check source folders");
  });

  it("distinguishes an empty library from one that has not loaded", () => {
    expect(statusLine({ ...IDLE, counts: null }).text).toBe("Starting…");
    expect(
      statusLine({ ...IDLE, counts: { images: [], videos: [], others: [] } }).text,
    ).toBe("Nothing to handle");
  });

  it("never shows a version number", () => {
    // Both versions left the main window deliberately: the app's belongs to
    // About, ffmpeg's to the tools modal.
    expect(statusLine(IDLE).text).not.toMatch(/\d+\.\d+\.\d+/);
  });
});
