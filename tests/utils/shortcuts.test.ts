import { describe, expect, it } from "vitest";
import { hasMod, isHelpShortcut } from "../../src/utils/shortcuts";
import { isZoomIn, isZoomOut, isZoomReset } from "../../src/utils/zoom";

// The detectors only read key/modifier fields, so a plain stub suffices in
// the node test environment (no DOM KeyboardEvent constructor).
function key(init: { key: string; metaKey?: boolean; ctrlKey?: boolean; altKey?: boolean }): KeyboardEvent {
  return {
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...init,
  } as KeyboardEvent;
}

describe("command modifier", () => {
  it("both Cmd and Ctrl fire on every platform", () => {
    expect(hasMod(key({ key: "/", metaKey: true }))).toBe(true);
    expect(hasMod(key({ key: "/", ctrlKey: true }))).toBe(true);
    expect(hasMod(key({ key: "/" }))).toBe(false);
  });

  it("help binds the chord under either modifier plus the bare Question alias", () => {
    expect(isHelpShortcut(key({ key: "/", metaKey: true }))).toBe(true);
    expect(isHelpShortcut(key({ key: "/", ctrlKey: true }))).toBe(true);
    expect(isHelpShortcut(key({ key: "?" }))).toBe(true);
    expect(isHelpShortcut(key({ key: "/" }))).toBe(false);
  });

  it("zoom chords accept either modifier and the JIS semicolon", () => {
    expect(isZoomIn(key({ key: "=", ctrlKey: true }))).toBe(true);
    expect(isZoomIn(key({ key: ";", metaKey: true }))).toBe(true);
    expect(isZoomOut(key({ key: "-", ctrlKey: true }))).toBe(true);
    expect(isZoomReset(key({ key: "0", metaKey: true }))).toBe(true);
    expect(isZoomIn(key({ key: "=" }))).toBe(false);
  });
});
