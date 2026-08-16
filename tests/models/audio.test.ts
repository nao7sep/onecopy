import { describe, expect, it } from "vitest";
import { isAudioFile, originalUrlByPath } from "../../src/models/items";

describe("audio detection", () => {
  it("recognizes the recorder and memo formats, case-insensitively", () => {
    for (const name of ["memo.m4a", "SONG.MP3", "raw.WAV", "talk.opus", "note.amr"]) {
      expect(isAudioFile(name), name).toBe(true);
    }
  });

  it("claims nothing else", () => {
    for (const name of ["photo.jpg", "clip.mov", "doc.pdf", "noext", ".m4a", "m4a"]) {
      expect(isAudioFile(name), name).toBe(false);
    }
  });
});

describe("the unhashed original route", () => {
  it("addresses a file by its path id", () => {
    // An audio memo with a unique size is never content-read, so it has no
    // hash — the path-id form is the only way the protocol can reach it.
    expect(originalUrlByPath(42)).toContain("path-42");
  });
});
