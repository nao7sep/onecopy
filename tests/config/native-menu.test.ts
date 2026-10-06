// OneCopy never uses macOS Spaces fullscreen, which slides windows sideways;
// fullscreen is its own raised borderless window (src-tauri/src/fullscreen.rs).
// The native menu and the page-load hook are built from a live app handle, so
// this reads their source: no menu offers the predefined Toggle Full Screen
// item, and every window refuses Spaces fullscreen as its page starts loading
// (the behaviour itself is pinned in src-tauri/tests/fullscreen_tests.rs).

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";

const menu = readFileSync(
  fileURLToPath(new URL("../../src-tauri/src/menu.rs", import.meta.url)),
  "utf8",
);
const lib = readFileSync(
  fileURLToPath(new URL("../../src-tauri/src/lib.rs", import.meta.url)),
  "utf8",
);

it("offers no Spaces fullscreen item in the native menu", () => {
  expect(menu).not.toMatch(/PredefinedMenuItem::fullscreen/);
  expect(menu).not.toContain("nativeMenu.fullscreen");
});

it("refuses Spaces fullscreen for every window as its page starts loading", () => {
  const hook = lib.slice(lib.indexOf(".on_page_load("), lib.indexOf(".menu(|app|"));
  expect(hook).toContain("PageLoadEvent::Started");
  expect(hook).toContain("fullscreen::refuse_spaces_fullscreen(&webview.window())");
});
