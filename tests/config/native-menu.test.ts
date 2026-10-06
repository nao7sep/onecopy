// OneCopy never uses macOS Spaces fullscreen, which slides windows sideways;
// fullscreen is its own raised borderless window (src-tauri/src/fullscreen.rs).
// The native menu is built from a live app handle, so this reads its source:
// no menu offers the predefined Toggle Full Screen item.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";

const menu = readFileSync(
  fileURLToPath(new URL("../../src-tauri/src/menu.rs", import.meta.url)),
  "utf8",
);

it("offers no Spaces fullscreen item in the native menu", () => {
  expect(menu).not.toMatch(/PredefinedMenuItem::fullscreen/);
  expect(menu).not.toContain("nativeMenu.fullscreen");
});
