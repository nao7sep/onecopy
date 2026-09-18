// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";

import { applyTheme, applyUiFont } from "../../src/utils/theme";
import { resetTauriMocks, setTheme } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks();
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    configurable: true,
    value: {},
  });
});

describe("window theme", () => {
  it("keeps native chrome aligned with explicit webview themes", async () => {
    applyTheme("dark");
    await vi.waitFor(() => expect(setTheme).toHaveBeenLastCalledWith("dark"));

    applyTheme("light");
    await vi.waitFor(() => expect(setTheme).toHaveBeenLastCalledWith("light"));
  });

  it("returns native chrome ownership to the OS for System", async () => {
    applyTheme("system");
    await vi.waitFor(() => expect(setTheme).toHaveBeenLastCalledWith(null));
  });
});

describe("UI font preference", () => {
  it("keeps CSS fallback ownership out of the user preference", () => {
    applyUiFont("Iosevka, monospace");
    expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe(
      "Iosevka, monospace",
    );

    applyUiFont(
      'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif',
    );
    expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe(
      "",
    );
  });
});
