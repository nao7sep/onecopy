// What the footer's managed-tools chip says, and how loudly.
//
// The managed-runtime-dependencies conventions make "Up to date" a silent
// default and warn against permanent benign FYIs. Two developer decisions
// (2026-08-17) shape the rest: an absent ffmpeg is a WARNING, not an FYI —
// without it every video and every HEIC is a placeholder — and the chip
// speaks for the whole registry, never as if ffmpeg were the only tool.

import { describe, expect, it } from "vitest";
import { toolsChip, type DependencyState } from "../../src/state/binaries-store";

function entry(id: string, status: DependencyState["status"]): DependencyState {
  return {
    id,
    label: id,
    kind: id === "ffmpeg" ? "binary" : "model",
    status,
    facts: { installedVersion: null, latestKnownVersion: null, lastCheckedAtUtc: null },
    path: "",
    checkable: id === "ffmpeg",
    released: id === "ffmpeg" ? null : "2024-10-01",
  };
}

describe("the footer's managed-tools chip", () => {
  it("says nothing when everything installed is fine", () => {
    expect(toolsChip(false, "", [entry("ffmpeg", "up-to-date")])).toBeNull();
  });

  it("says nothing when installed but never checked", () => {
    // Update checks default OFF, so this is where most installs sit forever —
    // showing it permanently would be exactly the nagging the convention names.
    expect(toolsChip(false, "", [entry("ffmpeg", "installed-unchecked")])).toBeNull();
  });

  it("says nothing before the state has loaded", () => {
    expect(toolsChip(false, "", [])).toBeNull();
  });

  it("warns with remedy-shaped copy when ffmpeg is missing", () => {
    // Absence is a capability hole (developer, 2026-08-17): the copy sells
    // the feature being unlocked, never the tool's internal name alone.
    const chip = toolsChip(false, "", [entry("ffmpeg", "not-installed")]);
    expect(chip?.role).toBe("warning");
    expect(chip?.text).toMatch(/video/i);
    expect(chip?.text).toMatch(/HEIC/);
  });

  it("stays silent for a missing MODEL", () => {
    // A missing model disables one enhancement, not a media kind; its own
    // feature surface names the remedy.
    expect(
      toolsChip(false, "", [entry("ffmpeg", "up-to-date"), entry("clip-vit-b32", "not-installed")]),
    ).toBeNull();
  });

  it("warns when any entry has an update, counting them honestly", () => {
    const one = toolsChip(false, "", [
      entry("ffmpeg", "up-to-date"),
      entry("clip-vit-b32", "update-available"),
    ]);
    expect(one?.role).toBe("warning");
    expect(one?.text).toBe("Tool update available");
    const two = toolsChip(false, "", [
      entry("ffmpeg", "update-available"),
      entry("clip-vit-b32", "update-available"),
    ]);
    expect(two?.text).toBe("Tool updates available");
  });

  it("shows progress while installing, whatever the stored states", () => {
    const chip = toolsChip(true, "downloading 40%", [entry("ffmpeg", "not-installed")]);
    expect(chip).toEqual({ text: "downloading 40%", role: "neutral" });
  });

  it("never puts a version number in the footer", () => {
    const statuses: DependencyState["status"][] = [
      "up-to-date",
      "installed-unchecked",
      "update-available",
      "not-installed",
    ];
    for (const status of statuses) {
      const chip = toolsChip(false, "", [entry("ffmpeg", status)]);
      if (chip !== null) expect(chip.text).not.toMatch(/\d+\.\d+/);
    }
  });
});
