// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from "vitest";
import { installCloseWindowShortcut } from "../../src/workflows/close-window-shortcut";
import { isCloseWindowShortcut } from "../../src/utils/shortcuts";

let uninstall: (() => void) | null = null;
afterEach(() => { uninstall?.(); uninstall = null; });

function install() {
  const close = vi.fn(() => Promise.resolve());
  uninstall = installCloseWindowShortcut(close);
  return close;
}

function press(init: KeyboardEventInit): KeyboardEvent {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  document.body.dispatchEvent(event);
  return event;
}

it("Cmd+W and Ctrl+W close this window and keep the key from the native menu", () => {
  const close = install();
  expect(press({ key: "w", metaKey: true }).defaultPrevented).toBe(true);
  expect(press({ key: "W", ctrlKey: true }).defaultPrevented).toBe(true);
  expect(close).toHaveBeenCalledTimes(2);
});

it("a held chord closes one window, not each window that becomes key after it", () => {
  const close = install();
  press({ key: "w", metaKey: true });
  expect(press({ key: "w", metaKey: true, repeat: true }).defaultPrevented).toBe(true);
  expect(close).toHaveBeenCalledTimes(1);
});

it("other chords, IME composition and keys a page already handled leave the window open", () => {
  const close = install();
  press({ key: "w" });
  press({ key: "w", metaKey: true, shiftKey: true });
  press({ key: "w", metaKey: true, altKey: true });
  press({ key: "w", metaKey: true, isComposing: true });
  const handled = new KeyboardEvent("keydown", { key: "w", metaKey: true, cancelable: true });
  handled.preventDefault();
  window.dispatchEvent(handled);
  expect(close).not.toHaveBeenCalled();
});

it("names Cmd/Ctrl+W only", () => {
  expect(isCloseWindowShortcut({ key: "w", metaKey: true, ctrlKey: false, altKey: false, shiftKey: false } as KeyboardEvent)).toBe(true);
  expect(isCloseWindowShortcut({ key: "q", metaKey: true, ctrlKey: false, altKey: false, shiftKey: false } as KeyboardEvent)).toBe(false);
});

it("in the Preview window, Cmd/Ctrl+W asks only that window to close, never Main", async () => {
  const { close, getCurrentWindow, setCurrentWindowLabel } = await import("../mocks/tauri");
  setCurrentWindowLabel("preview");
  close.mockClear();
  getCurrentWindow.mockClear();
  try {
    uninstall = installCloseWindowShortcut();
    press({ key: "w", metaKey: true });
    expect(getCurrentWindow.mock.results.map((result) => (result.value as { label: string }).label)).toEqual(["preview"]);
    expect(close).toHaveBeenCalledOnce();
  } finally {
    setCurrentWindowLabel("main");
  }
});
