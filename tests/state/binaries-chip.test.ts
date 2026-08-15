// What the footer says about a managed tool.
//
// The managed-runtime-dependencies conventions make "Up to date" a silent
// default and warn against permanent benign FYIs. The app previously rendered
// `ffmpeg 9.0` forever, which is both the wrong vocabulary (the convention's
// language is the four STATES, not a version) and the wrong display model.

import { describe, expect, it } from "vitest";
import { ffmpegChipText } from "../../src/state/binaries-store";

describe("the footer's managed-tool text", () => {
  it("says nothing when the tool is installed and fine", () => {
    expect(ffmpegChipText(false, "", "up-to-date")).toBeNull();
  });

  it("says nothing when installed but never checked", () => {
    // Update checks default OFF, so this is where most installs sit forever —
    // showing it permanently would be exactly the nagging the convention names.
    expect(ffmpegChipText(false, "", "installed-unchecked")).toBeNull();
  });

  it("says nothing before the state has loaded", () => {
    expect(ffmpegChipText(false, "", null)).toBeNull();
  });

  it("stays visible when the tool is missing", () => {
    // The convention's explicit exception: silence for an optional-absent
    // dependency "risks a dead feature". Here that is every video and every
    // HEIC photo reduced to a placeholder tile.
    expect(ffmpegChipText(false, "", "not-installed")).toMatch(/not installed/i);
  });

  it("warns when an update is available", () => {
    expect(ffmpegChipText(false, "", "update-available")).toMatch(/update/i);
  });

  it("shows progress while installing, whatever the stored state", () => {
    expect(ffmpegChipText(true, "downloading 40%", "not-installed")).toBe(
      "downloading 40%",
    );
    expect(ffmpegChipText(true, "verifying", "up-to-date")).toBe("verifying");
  });

  it("never puts a version number in the footer", () => {
    for (const status of ["up-to-date", "installed-unchecked", "update-available", "not-installed"]) {
      const text = ffmpegChipText(false, "", status);
      if (text !== null) expect(text).not.toMatch(/\d+\.\d+/);
    }
  });
});
