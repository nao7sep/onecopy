import { afterEach, describe, expect, it, vi } from "vitest";
import { hasMod, isHelpShortcut, isSettingsShortcut } from "../../src/utils/shortcuts";
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
  it("fires under either modifier", () => {
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

afterEach(() => {
  // Restore the suite-wide stub so module-load platform reads stay predictable.
  vi.stubGlobal("navigator", { platform: "", userAgent: "" });
  vi.resetModules();
});

describe("the platform word", () => {
  // tests/setup.ts stubs navigator.platform = "" for the node environment, and
  // no spec overrode it — so isApplePlatform was permanently false suite-wide
  // and primaryModWord(), the ONE platform-dependent export, had no test.
  it("reads Cmd on Apple platforms", async () => {
    vi.stubGlobal("navigator", { platform: "MacIntel", userAgent: "" });
    vi.resetModules();
    const { primaryModWord } = await import("../../src/utils/shortcuts");
    expect(primaryModWord()).toBe("Cmd");
  });

  it("reads Ctrl elsewhere", async () => {
    vi.stubGlobal("navigator", { platform: "Win32", userAgent: "" });
    vi.resetModules();
    const { primaryModWord } = await import("../../src/utils/shortcuts");
    expect(primaryModWord()).toBe("Ctrl");
  });
});

describe("the settings chord", () => {
  it("binds Cmd/Ctrl+Comma and nothing else", () => {
    expect(isSettingsShortcut(key({ key: ",", metaKey: true }))).toBe(true);
    expect(isSettingsShortcut(key({ key: ",", ctrlKey: true }))).toBe(true);
    expect(isSettingsShortcut(key({ key: "," }))).toBe(false);
    expect(isSettingsShortcut(key({ key: ".", metaKey: true }))).toBe(false);
  });
});
