import { describe, expect, it } from "vitest";
import { libraryLine, statusLine } from "../../src/models/status";
import type { SectionCounts } from "../../src/models/sections";

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
  scanning: false,
  progress: "",
  rescanNeeded: false,
  counts: COUNTS,
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
      progress: "previews: 30/900",
      rescanNeeded: true,
    });
    expect(status.tone).toBe("danger");
    expect(status.text).toContain("could not be deleted");
  });

  it("prefers live scan progress over the totals it is busy changing", () => {
    const status = statusLine({ ...IDLE, scanning: true, progress: "previews: 30/900" });
    expect(status.text).toBe("previews: 30/900");
  });

  it("still says something while a scan has yet to report a phase", () => {
    expect(statusLine({ ...IDLE, scanning: true, progress: "" }).text).toBe("Scanning…");
  });

  it("warns that the index is knowingly incomplete", () => {
    const status = statusLine({ ...IDLE, rescanNeeded: true });
    expect(status.tone).toBe("warning");
    expect(status.title).toContain("Scan all sources");
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
