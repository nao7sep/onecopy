// @vitest-environment happy-dom
//
// The main window's two fixed bands. The title band at the top owns the app's
// name and its one menu; the footer below owns standing state only.
//
// Two of these assertions pin a requirement that is invisible in the code once
// it is met and easy to undo by accident: the menu must be reachable from the
// TITLE band (a menu that quietly moved back to the footer still passes any
// "the menu exists" check), and the version must not appear in the main window
// at all — it belongs to About, and a permanent version number is not standing
// state. The third pins that the derived window minimum actually reserves the
// band, which is what stops the footer being overlapped at the smallest size.

import { beforeEach, afterEach, describe, expect, it } from "vitest";
import { render, cleanup, act } from "@testing-library/react";
import App from "../../src/App";
import { useAppStore } from "../../src/state/app-store";
import { computeMinWindowHeight, HEADER_HEIGHT } from "../../src/utils/windowSizing";
import { isMaximized, mockCommands, onMoved, resetTauriMocks, setMinSize } from "../mocks/tauri";

beforeEach(() => {
  // Stores wire their event listeners once at module load, so those survive.
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    get_section_counts: () => ({ images: [], videos: [], others: [] }),
    get_issues: () => ({ issues: [], total: 0 }),
    binaries_state: () => null,
    patch_state: () => ({}),
    log_event: () => null,
    logging_debug_enabled: () => false,
  });
});

afterEach(() => cleanup());

describe("the title band", () => {
  it("carries the app name and the menu trigger", () => {
    const view = render(<App />);
    const header = view.container.querySelector("header");
    expect(header).not.toBeNull();
    expect(header?.textContent).toContain("OneCopy");
    // The trigger must live INSIDE the band, not merely somewhere in the app.
    expect(header?.querySelector('[aria-label="Open menu"]')).not.toBeNull();
  });

  it("leaves the footer to standing state alone", () => {
    const view = render(<App />);
    const footer = view.container.querySelector("footer");
    expect(footer).not.toBeNull();
    expect(footer?.querySelector('[aria-label="Open menu"]')).toBeNull();
    expect(footer?.textContent).not.toContain("OneCopy");
  });

  it("keeps the version out of the main window entirely", () => {
    const view = render(<App />);
    // Guard against a vacuous pass: an empty container matches no regex.
    expect(view.container.textContent).toContain("OneCopy");
    // Any dotted version triple anywhere in the shell fails this — the About
    // modal is where the number lives, and it is not rendered here.
    expect(view.container.textContent).not.toMatch(/\d+\.\d+\.\d+/);
  });

  it("is reserved in the window minimum, not overlapped by the content", async () => {
    render(<App />);
    // The effect runs on mount but checks isMaximized first (a maximized
    // window defers the constraint), so the call is a tick away.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    const [size] = setMinSize.mock.calls[0] ?? [];
    expect(size?.height).toBe(computeMinWindowHeight());
    expect(HEADER_HEIGHT).toBeGreaterThan(0);
  });
});

describe("the maximized main window (the developer's normal state)", () => {
  const drain = () =>
    act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

  it("defers the min-size constraint instead of applying it", async () => {
    // On Windows the min-size call knocks a maximized window back to normal
    // — pressing Space (which re-derives the minimum for the split pane)
    // un-maximized the developer's window. A maximized window cannot go
    // below any minimum, so the constraint must WAIT.
    isMaximized.mockResolvedValue(true);
    try {
      render(<App />);
      await drain();
      expect(setMinSize).not.toHaveBeenCalled();
    } finally {
      isMaximized.mockResolvedValue(false);
    }
  });

  it("applies the min size normally when not maximized", async () => {
    render(<App />);
    await drain();
    expect(setMinSize).toHaveBeenCalled();
  });

  it("saves maximized as a FLAG, never as geometry", async () => {
    // Writing the maximized rect into windowBounds would overwrite the
    // remembered normal size — un-maximizing would have nowhere to return.
    let movedHandler: (() => void) | null = null;
    onMoved.mockImplementation(async (handler: unknown) => {
      movedHandler = handler as () => void;
      return () => {};
    });
    // patchState needs a live appData; the NORMAL bounds were saved earlier.
    const normal = { x: 10, y: 20, width: 1400, height: 900 };
    useAppStore.setState({
      appData: {
        config: { sourceDirs: [], defaultTimezone: "UTC" },
        state: { windowBounds: normal },
        dataRoot: "/data",
        debugEnabled: false,
      } as never,
    });
    isMaximized.mockResolvedValue(true);
    try {
      render(<App />);
      await drain();
      expect(movedHandler).not.toBeNull();
      movedHandler!();
      // Past the 500ms save debounce (patchState publishes optimistically,
      // so the store state is authoritative here).
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 600));
      });
      const state = useAppStore.getState().appData?.state as Record<string, unknown>;
      expect(state.windowMaximized).toBe(true);
      // The remembered normal geometry survived the maximized save.
      expect(state.windowBounds).toEqual(normal);
    } finally {
      isMaximized.mockResolvedValue(false);
      onMoved.mockImplementation(async (_handler: unknown) => () => {});
      useAppStore.setState({ appData: null });
    }
  });
});
