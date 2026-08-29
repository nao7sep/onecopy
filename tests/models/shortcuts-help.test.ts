// Does the help sheet describe the app that exists?
//
// A hand-maintained shortcut list drifts silently — nothing links a printed
// chord to a live binding — and a row that outlives its key reads as a broken
// app rather than a stale list. These specs check the two things a list can be
// wrong about on its own: naming a key nothing handles, and omitting one the
// app does handle.
//
// They read the SOURCE of the handlers rather than driving each surface. That
// is deliberate: a chord like Shift+Enter in the destinations tree needs a
// populated tree, a selection and a destination root to fire, so driving it
// would test the fixture. Reading the binding sites catches the actual failure
// mode here, which is a row nobody updated.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { shortcutGroups } from "../../src/models/shortcuts";

const SOURCES = [
  "src/App.tsx",
  "src/hooks/useGlobalCommands.ts",
  "src/hooks/useMainWindowLifecycle.ts",
  "src/components/Grid.tsx",
  "src/state/quick-view-store.ts",
  "src/components/DestinationsTab.tsx",
  "src/components/ComparisonView.tsx",
  "src/components/InspectableImage.tsx",
  "src/components/Sidebar.tsx",
  "src/windows/PreviewWindow.tsx",
  "src/windows/ComparisonWindow.tsx",
  "src/state/comparison-store.ts",
  "src/utils/shortcuts.ts",
  "src/utils/zoom.ts",
].map((path) => readFileSync(path, "utf8"));

const ALL_SOURCE = SOURCES.join("\n");

function handles(...needles: string[]): boolean {
  return needles.some((needle) => ALL_SOURCE.includes(needle));
}

const rows = shortcutGroups().flatMap((group) =>
  group.rows.map((row) => ({ ...row, group: group.title })),
);

/** The binding that makes each printed chord real. A row added to the sheet
 * with no entry here fails the completeness spec below, which is the point:
 * the sheet cannot grow a row without someone naming what implements it. */
const EVIDENCE: Record<string, () => boolean> = {
  Arrows: () => handles('"ArrowRight"', '"ArrowDown"'),
  "Home / End": () => handles('"Home"') && handles('"End"'),
  "Page Up / Page Down": () => handles('"PageUp"') && handles('"PageDown"'),
  "Shift+Arrows": () => handles("event.shiftKey"),
  Click: () => handles("event.detail"),
  "Shift+Click": () => handles("rangeSelect"),
  Space: () => handles('event.key === " "'),
  Enter: () => handles('"Enter"'),
  F: () => handles('toLowerCase() === "f"'),
  "Delete / Backspace": () => handles('"Delete" || event.key === "Backspace"'),
  "Shift+Delete": () => handles("setConfirmPermanent"),
  "1–9 / 0 / A–F": () => handles("slotIndexForKey"),
  "Left / Right": () => handles("nextPage()", "prevPage()"),
  S: () => handles("toggleShortlist"),
  "Shift+1–9/0/A–F": () => handles("slotIndexForShiftedCode"),
  "Shift+Enter": () => handles("shiftKey"),
  Escape: () => handles('"Escape"'),
  R: () => handles("isSectionRecheckShortcut"),
  Comma: () => handles("isSettingsShortcut"),
  "Slash / Question": () => handles("isHelpShortcut"),
  "Equal/Plus/Semicolon": () => handles("isZoomIn"),
  Minus: () => handles("isZoomOut"),
  "0": () => handles("isZoomReset"),
};

/** Strips the platform modifier so one entry covers Cmd and Ctrl alike. */
function evidenceKey(chord: string): string {
  return chord.replace(/^(Cmd|Ctrl)\+/, "");
}

describe("every chord the help sheet prints", () => {
  it.each(rows)("$group: $chord is actually bound", ({ chord }) => {
    const check = EVIDENCE[evidenceKey(chord)];
    expect(check, `no evidence entry for "${chord}"`).toBeDefined();
    expect(check!(), `"${chord}" is printed but nothing handles it`).toBe(true);
  });
});

describe("keys the app handles but the sheet forgot", () => {
  const printed = new Set(rows.map((r) => evidenceKey(r.chord)));

  it("lists Space as Quick View rather than a selection or Preview toggle", () => {
    expect(printed.has("Space")).toBe(true);
  });

  it("lists the Settings chord, which left the menu and has nowhere else to be seen", () => {
    // Removing the chord hints from the hamburger menu made this sheet the
    // ONLY place Cmd+Comma is discoverable.
    expect(printed.has("Comma")).toBe(true);
  });

  it("lists the preview window's Escape, whose partner F was already printed", () => {
    // Bound in PreviewWindow (leave fullscreen, else close) and unprinted
    // until the convention's catalogue rule was checked in both directions.
    const looking = shortcutGroups().find((g) => g.title === "Looking");
    expect(looking?.rows.some((r) => r.chord === "Escape")).toBe(true);
  });

  it("does not print a chord for anything removed", () => {
    // Persistent Preview has no settled keyboard shortcut; its visibility is
    // controlled by chrome while Space belongs to transient Quick View.
    const actions = rows.map((r) => r.action.toLowerCase());
    expect(actions.some((a) => a.includes("follows-selection"))).toBe(false);
    expect(rows.some((r) => r.chord === "P")).toBe(false);
  });

  it("spells the comparison paging keys as words", () => {
    const comparisonView = readFileSync("src/components/ComparisonView.tsx", "utf8");
    expect(comparisonView).toContain("Left/Right pages");
    expect(comparisonView).not.toContain("←/→ pages");
  });
});
