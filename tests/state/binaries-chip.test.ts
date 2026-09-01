// What the footer's managed-tools chip says, and how loudly.
//
// "Up to date" stays silent, warnings always show, and — the fleet decision of
// 2026-08-21, superseding the earlier all-silent informational tuning — a present
// ffmpeg whose currency is unknown shows PERMANENTLY in normal muted ink, so one
// standing path always says the tools may be stale. Two earlier decisions
// (2026-08-17) still shape the rest: an absent ffmpeg is a WARNING, not an FYI —
// without it every video and every HEIC is a placeholder — and the chip speaks
// for the whole registry, never as if ffmpeg were the only tool. Absent OPTIONAL
// models stay off the chip; their features surface the need at point of use.

import { describe, expect, it } from "vitest";
import { toolsChip, type DependencyState } from "../../src/state/binaries-store";

function entry(id: string, status: DependencyState["status"]): DependencyState {
  return {
    id,
    label: id,
    kind: id === "ffmpeg" ? "binary" : "model",
    status,
    // Read from the artifact: a present entry reports itself unless a test
    // overrides it to model the unreadable case.
    installedVersion: status === "not-installed" ? null : "9.0",
    facts: { latestKnownVersion: null, lastCheckedAtUtc: null },
    path: "",
    requiredForCore: id === "ffmpeg",
    checkable: id === "ffmpeg",
    released: id === "ffmpeg" ? null : "2024-10-01",
  };
}

describe("the footer's managed-tools chip", () => {
  it("says nothing when everything installed is fine", () => {
    expect(toolsChip(false, "", [entry("ffmpeg", "up-to-date")])).toBeNull();
  });

  it("shows a calm, normal-ink line when installed but never checked", () => {
    // Update checks default OFF, so many installs sit here — the line is the one
    // standing path to notice that, in neutral ink so it never nags as a warning.
    expect(toolsChip(false, "", [entry("ffmpeg", "installed-unchecked")])).toEqual({
      text: "Tools not checked",
      role: "neutral",
    });
  });

  it("names the unreadable case, which only re-acquiring can clear", () => {
    const unreadable = { ...entry("ffmpeg", "installed-unchecked"), installedVersion: null };
    expect(toolsChip(false, "", [unreadable])).toEqual({
      text: "Tool version unreadable",
      role: "neutral",
    });
  });

  it("keeps absent optional models off the chip", () => {
    expect(
      toolsChip(false, "", [entry("ffmpeg", "up-to-date"), entry("whisper-large-v3-turbo", "not-installed")]),
    ).toBeNull();
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
      toolsChip(false, "", [entry("ffmpeg", "up-to-date"), entry("whisper-large-v3-turbo", "not-installed")]),
    ).toBeNull();
  });

  it("warns when any entry has an update, counting them honestly", () => {
    const one = toolsChip(false, "", [
      entry("ffmpeg", "up-to-date"),
      entry("whisper-large-v3-turbo", "update-available"),
    ]);
    expect(one?.role).toBe("warning");
    expect(one?.text).toBe("Tool update available");
    const two = toolsChip(false, "", [
      entry("ffmpeg", "update-available"),
      entry("whisper-large-v3-turbo", "update-available"),
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
