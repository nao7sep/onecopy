// The status bar speaks user language: every pipeline token has a friendly
// label, and an unknown one degrades to readable instead of raw.

import { describe, expect, it } from "vitest";
import { phaseLabel, progressLine } from "../../src/models/scan";

describe("scan phase labels", () => {
  it("covers every phase the pipeline emits", () => {
    // The core's tokens, in pipeline order. A new phase added there without
    // a label here falls back to bare capitalization — readable, but this
    // list is the reminder to choose real words.
    const tokens = [
      "walk", "hash", "extract", "resolve", "pair",
      "derive", "video", "embed", "faces", "group",
    ];
    for (const token of tokens) {
      const label = phaseLabel(token);
      expect(label).not.toBe(token);
      expect(label[0]).toBe(label[0].toUpperCase());
    }
    expect(phaseLabel("derive")).toBe("Making previews");
    expect(phaseLabel("group")).toBe("Grouping similar shots");
  });

  it("degrades an unknown token to a capitalized word", () => {
    expect(phaseLabel("transmogrify")).toBe("Transmogrify");
  });

  it("joins label and detail with a dash so colons in details still read", () => {
    expect(progressLine("walk", "/photos: 812 files (12 new)")).toBe(
      "Scanning \u2014 /photos: 812 files (12 new)",
    );
  });
});
