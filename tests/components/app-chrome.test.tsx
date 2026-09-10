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

import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { render, cleanup, act, fireEvent } from "@testing-library/react";
import { ReadyApp } from "../../src/App";
import { retainStatePatch, useAppStore } from "../../src/state/app-store";
import type { LoadedAppData } from "../../src/repositories";
import { computeMinWindowHeight, HEADER_HEIGHT } from "../../src/utils/windowSizing";
import {
  isMaximized,
  invokeCalls,
  mockCommands,
  onCloseRequested,
  onResized,
  resetTauriMocks,
  setMinSize,
} from "../mocks/tauri";

const READY_APP_DATA: LoadedAppData = {
  config: { sourceDirs: [], defaultTimezone: "UTC" },
  state: {},
  dataRoot: "/data",
  debugEnabled: false,
  quarantines: [],
};

function renderReadyApp() {
  return render(<ReadyApp appData={useAppStore.getState().appData ?? READY_APP_DATA} />);
}

beforeEach(() => {
  // Stores wire their event listeners once at module load, so those survive.
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    get_section_counts: () => ({ images: [], videos: [], others: [] }),
    get_issues: () => ({ issues: [], total: 0 }),
    binaries_state: () => null,
    patch_state: () => ({}),
    request_app_exit: () => null,
    log_event: () => null,
    logging_debug_enabled: () => false,
  });
  useAppStore.setState({
    appData: READY_APP_DATA,
    startupFailure: null,
    quarantines: [],
  });
});

afterEach(() => cleanup());

describe("the title band", () => {
  it("carries the app name and the menu trigger", () => {
    const view = renderReadyApp();
    const header = view.container.querySelector("header");
    expect(header).not.toBeNull();
    expect(header?.textContent).toContain("OneCopy");
    // The trigger must live INSIDE the band, not merely somewhere in the app.
    expect(header?.querySelector('[aria-label="Open menu"]')).not.toBeNull();
  });

  it("draws the zoom marks without changing their accessible names", () => {
    const view = renderReadyApp();
    fireEvent.click(view.getByRole("button", { name: "Open menu" }));

    for (const name of ["Zoom out", "Zoom in"]) {
      const button = view.getByRole("button", { name });
      expect(button.querySelector("svg")).not.toBeNull();
      expect(button.textContent).toBe("");
    }
  });

  it("exposes Activity trace independently of the developer gate", () => {
    const releaseView = renderReadyApp();
    fireEvent.click(releaseView.getByRole("button", { name: "Open menu" }));
    expect(releaseView.getByText("Activity trace…")).toBeTruthy();
    releaseView.unmount();

    useAppStore.setState({
      appData: { ...READY_APP_DATA, debugEnabled: true },
    });
    const debugView = renderReadyApp();
    fireEvent.click(debugView.getByRole("button", { name: "Open menu" }));
    expect(debugView.getByText("Activity trace…")).toBeTruthy();
  });

  it("leaves the footer to standing state alone", () => {
    const view = renderReadyApp();
    const footer = view.container.querySelector("footer");
    expect(footer).not.toBeNull();
    expect(footer?.querySelector('[aria-label="Open menu"]')).toBeNull();
    expect(footer?.textContent).not.toContain("OneCopy");
  });

  it("keeps the version out of the main window entirely", () => {
    const view = renderReadyApp();
    // Guard against a vacuous pass: an empty container matches no regex.
    expect(view.container.textContent).toContain("OneCopy");
    // Any dotted version triple anywhere in the shell fails this — the About
    // modal is where the number lives, and it is not rendered here.
    expect(view.container.textContent).not.toMatch(/\d+\.\d+\.\d+/);
  });

  it("is reserved in the window minimum, not overlapped by the content", async () => {
    renderReadyApp();
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

describe("the dynamic window minimum", () => {
  const drain = () =>
    act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

  it("defers the min-size constraint instead of applying it", async () => {
    // On Windows the min-size call knocks a maximized window back to normal
    // — showing the split Preview (which re-derives the minimum)
    // un-maximized the developer's window. A maximized window cannot go
    // below any minimum, so the constraint must WAIT.
    isMaximized.mockResolvedValue(true);
    let resizedHandler: (() => void) | null = null;
    onResized.mockImplementation(async (handler: unknown) => {
      resizedHandler = handler as () => void;
      return () => {};
    });
    try {
      renderReadyApp();
      await drain();
      expect(setMinSize).not.toHaveBeenCalled();
      expect(resizedHandler).not.toBeNull();
      isMaximized.mockResolvedValue(false);
      await act(async () => {
        resizedHandler?.();
        await new Promise((resolve) => setTimeout(resolve, 0));
      });
      expect(setMinSize).toHaveBeenCalledOnce();
    } finally {
      isMaximized.mockResolvedValue(false);
      onResized.mockImplementation(async (_handler: unknown) => () => {});
    }
  });

  it("applies the min size normally when not maximized", async () => {
    renderReadyApp();
    await drain();
    expect(setMinSize).toHaveBeenCalled();
  });

});

describe("application close", () => {
  it("flushes interface state before requesting the Rust shutdown path", async () => {
    let closeHandler: ((event: { preventDefault: () => void }) => Promise<void>) | null = null;
    onCloseRequested.mockImplementation(async (handler: unknown) => {
      closeHandler = handler as typeof closeHandler;
      return () => {};
    });
    const preventDefault = vi.fn();
    renderReadyApp();
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 600));
    });

    expect(closeHandler).not.toBeNull();
    retainStatePatch({ zoomLevel: 1.2 });
    await act(async () => {
      await closeHandler?.({ preventDefault });
    });

    expect(preventDefault).toHaveBeenCalledOnce();
    const commands = invokeCalls.map((call) => call.command);
    expect(commands.indexOf("patch_state")).toBeGreaterThanOrEqual(0);
    expect(commands.indexOf("patch_state")).toBeLessThan(
      commands.indexOf("request_app_exit"),
    );
  });
});
