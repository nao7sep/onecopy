// The status bar speaks user language: every pipeline token has a friendly
// label, and an unknown one degrades to readable instead of raw.

import { describe, expect, it } from "vitest";
import {
  phaseLabel,
  progressLine,
  progressTitle,
  type ScanProgress,
} from "../../src/models/scan";

function progress(overrides: Partial<ScanProgress> = {}): ScanProgress {
  return {
    phase: "walk",
    done: 0,
    total: 1,
    currentPath: null,
    discovered: null,
    bytesDone: null,
    bytesTotal: null,
    failures: 0,
    nextPhase: "hash",
    ...overrides,
  };
}

describe("scan phase labels", () => {
  it("covers every phase the pipeline emits", () => {
    // The core's tokens, in pipeline order. A new phase added there without
    // a label here falls back to bare capitalization — readable, but this
    // list is the reminder to choose real words.
    const tokens = ["walk", "hash", "extract", "resolve", "pair", "indexed"];
    for (const token of tokens) {
      const label = phaseLabel(token);
      expect(label).not.toBe(token);
      expect(label[0]).toBe(label[0].toUpperCase());
    }
    expect(phaseLabel("extract")).toBe("Reading metadata");
    expect(phaseLabel("indexed")).toBe("Indexed");
  });

  it("degrades an unknown token to a capitalized word", () => {
    expect(phaseLabel("transmogrify")).toBe("Transmogrify");
  });

  it("formats stable source progress without inventing a file total", () => {
    expect(
      progressLine(
        progress({ total: 2, currentPath: "/photos", discovered: 812 }),
      ),
    ).toBe("Checking source folders \u2014 source 1/2 · 812 files found · /photos");
  });

  it("shows streamed byte percentage, failures, and the explicit next phase", () => {
    expect(
      progressLine(
        progress({
          phase: "hash",
          done: 12,
          total: 40,
          currentPath: "C:\\photos\\large.mov",
          bytesDone: 55,
          bytesTotal: 100,
          failures: 2,
          nextPhase: "extract",
        }),
      ),
    ).toBe("Reading files — 12/40 · large.mov · 55% · 2 failed");
    expect(
      progressLine(
        progress({ phase: "pair", done: 3, total: 3, nextPhase: "indexed" }),
      ),
    ).toBe("Pairing companions — 3/3 · Next: Indexed");
  });

  it("explains metadata scope and keeps derived work outside indexing", () => {
    expect(progressTitle(progress({ phase: "extract", nextPhase: "resolve" }))).toContain(
      "without decoding image pixels or video frames",
    );
    expect(progressTitle(progress({ phase: "indexed", nextPhase: null }))).toContain(
      "Background work",
    );
  });

  it("claims Up to date only when the index pass has no failures", () => {
    expect(
      progressLine(
        progress({ phase: "indexed", done: 1, total: 1, nextPhase: null }),
      ),
    ).toBe("Up to date");
    expect(
      progressLine(
        progress({
          phase: "indexed",
          done: 1,
          total: 1,
          failures: 2,
          nextPhase: null,
        }),
      ),
    ).toBe("Indexed — 2 failed · open Issues");
  });
});
