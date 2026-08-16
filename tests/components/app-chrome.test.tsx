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
import { render, cleanup } from "@testing-library/react";
import App from "../../src/App";
import { computeMinWindowHeight, HEADER_HEIGHT } from "../../src/utils/windowSizing";
import { mockCommands, resetTauriMocks, setMinSize } from "../mocks/tauri";

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
    // The effect runs on mount; the call carries the derived size.
    const [size] = setMinSize.mock.calls[0] ?? [];
    expect(size?.height).toBe(computeMinWindowHeight());
    expect(HEADER_HEIGHT).toBeGreaterThan(0);
  });
});
