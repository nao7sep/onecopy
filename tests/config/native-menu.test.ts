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

// The native About panel shows only the name and version, so the About item
// is OneCopy's own and opens Main's About surface, as the Settings item opens
// Settings (the listener is tested in tests/hooks/use-global-commands.test.tsx).
it("routes the About item to Main's About surface instead of the native panel", () => {
  expect(menu).not.toMatch(/PredefinedMenuItem::about/);
  expect(menu).toMatch(/MenuItem::with_id\(app, OPEN_ABOUT_MENU_ID/);
  expect(menu).toMatch(/OPEN_ABOUT_MENU_ID => Some\("menu:\/\/open-about"\)/);
  expect(lib).toContain("menu::main_surface_event(event.id().as_ref())");
});
