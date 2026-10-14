import { afterEach, describe, expect, it, vi } from "vitest";
import {
  hasMod,
  isHelpShortcut,
  isSectionRecheckShortcut,
  isSelectAllShortcut,
  isSettingsShortcut,
} from "../../src/utils/shortcuts";
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

  it("stays quiet under AltGr (Ctrl+Alt on Windows) so typed characters keep typing", () => {
    // Hungarian AltGr+Comma produces ";" — a zoom-in key — and unmapped AltGr
    // combos fall back to the base letter; neither may fire an accelerator.
    expect(hasMod(key({ key: ";", ctrlKey: true, altKey: true }))).toBe(false);
    expect(hasMod(key({ key: "/", ctrlKey: true, altKey: true }))).toBe(false);
    // Plain Alt without a command modifier never counted; still doesn't.
    expect(hasMod(key({ key: "/", altKey: true }))).toBe(false);
  });

  it("rejects AltGr+Enter as a command modifier", () => {
    // A raw Ctrl check turns Windows AltGr (Ctrl+Alt) into an application
    // command while the user is only typing.
    expect(hasMod(key({ key: "Enter", ctrlKey: true, altKey: true }))).toBe(false);
    expect(hasMod(key({ key: "Enter", metaKey: true, altKey: true }))).toBe(false);
    // Either primary modifier alone remains a command chord.
    expect(hasMod(key({ key: "Enter", metaKey: true }))).toBe(true);
    expect(hasMod(key({ key: "Enter", ctrlKey: true }))).toBe(true);
  });

  it("help binds the chord under either modifier plus the bare Question alias", () => {
    expect(isHelpShortcut(key({ key: "/", metaKey: true }))).toBe(true);
    expect(isHelpShortcut(key({ key: "/", ctrlKey: true }))).toBe(true);
    expect(isHelpShortcut(key({ key: "?" }))).toBe(true);
    expect(isHelpShortcut(key({ key: "/" }))).toBe(false);
    // An AltGr combo that happens to produce "?" is typing, not the alias —
    // the branch's own !altKey check keeps it quiet now that hasMod ignores
    // Ctrl+Alt chords.
    expect(isHelpShortcut(key({ key: "?", ctrlKey: true, altKey: true }))).toBe(false);
  });

  it("zoom chords accept either modifier and the JIS semicolon", () => {
    expect(isZoomIn(key({ key: "=", ctrlKey: true }))).toBe(true);
    expect(isZoomIn(key({ key: ";", metaKey: true }))).toBe(true);
    expect(isZoomOut(key({ key: "-", ctrlKey: true }))).toBe(true);
    expect(isZoomReset(key({ key: "0", metaKey: true }))).toBe(true);
    expect(isZoomIn(key({ key: "=" }))).toBe(false);
  });

  it("zoom chords reject AltGr so layouts that type zoom keys via AltGr keep typing", () => {
    expect(isZoomIn(key({ key: ";", ctrlKey: true, altKey: true }))).toBe(false);
    expect(isZoomIn(key({ key: "=", ctrlKey: true, altKey: true }))).toBe(false);
    expect(isZoomOut(key({ key: "-", ctrlKey: true, altKey: true }))).toBe(false);
    expect(isZoomReset(key({ key: "0", ctrlKey: true, altKey: true }))).toBe(false);
  });
});

afterEach(() => {
  // Restore the suite-wide stub so module-load platform reads stay predictable.
  vi.stubGlobal("navigator", { platform: "", userAgent: "" });
  vi.resetModules();
});

describe("the platform-bound exports", () => {
  // tests/setup.ts stubs navigator.platform = "" for the node environment, so
  // without a stub of their own these read the non-Apple answer suite-wide.
  async function onPlatform(platform: string) {
    vi.stubGlobal("navigator", { platform, userAgent: "" });
    vi.resetModules();
    return import("../../src/utils/shortcuts");
  }

  it("reads Cmd and Finder on Apple platforms", async () => {
    const { primaryModWord, fileManagerWord } = await onPlatform("MacIntel");
    expect(primaryModWord()).toBe("Cmd");
    expect(fileManagerWord()).toBe("Finder");
  });

  it("reads Ctrl and Explorer elsewhere", async () => {
    const { primaryModWord, fileManagerWord } = await onPlatform("Win32");
    expect(primaryModWord()).toBe("Ctrl");
    expect(fileManagerWord()).toBe("Explorer");
  });

  it("leaves Ctrl to the macOS text system and keeps the Cmd chord", async () => {
    const { shadowsMacTextEditing } = await onPlatform("MacIntel");
    expect(shadowsMacTextEditing(key({ key: "a", ctrlKey: true }))).toBe(true);
    expect(shadowsMacTextEditing(key({ key: "a", metaKey: true }))).toBe(false);
    expect(shadowsMacTextEditing(key({ key: "a", metaKey: true, ctrlKey: true }))).toBe(false);
  });

  it("never stands Ctrl down off macOS, where it is the command modifier", async () => {
    const { shadowsMacTextEditing } = await onPlatform("Win32");
    expect(shadowsMacTextEditing(key({ key: "a", ctrlKey: true }))).toBe(false);
  });
});

describe("the settings chord", () => {
  it("binds Cmd/Ctrl+Comma and nothing else", () => {
    expect(isSettingsShortcut(key({ key: ",", metaKey: true }))).toBe(true);
    expect(isSettingsShortcut(key({ key: ",", ctrlKey: true }))).toBe(true);
    expect(isSettingsShortcut(key({ key: "," }))).toBe(false);
    expect(isSettingsShortcut(key({ key: ".", metaKey: true }))).toBe(false);
    expect(isSettingsShortcut(key({ key: ",", ctrlKey: true, altKey: true }))).toBe(false);
  });
});

describe("the section recheck chord", () => {
  it("binds Cmd/Ctrl+R without stealing modified variants", () => {
    expect(isSectionRecheckShortcut(key({ key: "r", metaKey: true }))).toBe(true);
    expect(isSectionRecheckShortcut(key({ key: "R", ctrlKey: true }))).toBe(true);
    expect(isSectionRecheckShortcut(key({ key: "r" }))).toBe(false);
    expect(isSectionRecheckShortcut(key({ key: "r", ctrlKey: true, altKey: true }))).toBe(false);
    expect(
      isSectionRecheckShortcut({
        ...key({ key: "r", metaKey: true }),
        shiftKey: true,
      }),
    ).toBe(false);
  });
});

describe("the select-all chord", () => {
  it("binds Cmd/Ctrl+A without stealing AltGr or Shift variants", () => {
    expect(isSelectAllShortcut(key({ key: "a", metaKey: true }))).toBe(true);
    expect(isSelectAllShortcut(key({ key: "A", ctrlKey: true }))).toBe(true);
    expect(isSelectAllShortcut(key({ key: "a" }))).toBe(false);
    expect(isSelectAllShortcut(key({ key: "a", ctrlKey: true, altKey: true }))).toBe(false);
    expect(isSelectAllShortcut({ ...key({ key: "a", metaKey: true }), shiftKey: true })).toBe(false);
  });
});
