// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import SettingsModal from "../../src/components/SettingsModal";
import { useSettingsStore } from "../../src/state/settings-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

const config = {
  sourceDirs: ["C:\\Photos"],
  defaultTimezone: "Asia/Tokyo",
};

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    similar_exclusions_count: () => 0,
    cancel_cache_move: () => true,
  });
  useSettingsStore.getState().openWith(config);
});

afterEach(() => {
  useSettingsStore.setState({
    open: false,
    draft: null,
    opened: null,
    saving: false,
    movingCache: null,
    cancellingCacheMove: false,
  });
  cleanup();
});

describe("Settings categories", () => {
  it("uses keyboard-operable tabs instead of one long mixed scroller", () => {
    render(<SettingsModal />);
    expect(screen.getByRole("tabpanel").getAttribute("id")).toBe("settings-panel-library");
    expect(screen.getByText("Directories")).toBeTruthy();
    expect(screen.queryByText("Previews")).toBeNull();

    const library = screen.getByRole("tab", { name: "Library" });
    fireEvent.keyDown(library, { key: "ArrowRight" });

    expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Media" }));
    expect(screen.getByRole("tabpanel").getAttribute("id")).toBe("settings-panel-media");
    expect(screen.getByText("Previews")).toBeTruthy();
    expect(screen.queryByText("Directories")).toBeNull();
  });
});

describe("cache relocation", () => {
  it("blocks the parent close paths and exposes a real cancellable progress dialog", async () => {
    useSettingsStore.setState({
      saving: true,
      movingCache: { copiedBytes: 1_048_576, totalBytes: 2_097_152 },
    });
    render(<SettingsModal />);

    const dialogs = screen.getAllByRole("dialog");
    expect(dialogs).toHaveLength(2);
    const cancelButtons = screen.getAllByRole("button", { name: "Cancel" });
    expect(cancelButtons.some((button) => button.hasAttribute("disabled"))).toBe(true);
    const activeCancel = cancelButtons.find((button) => !button.hasAttribute("disabled"));
    expect(activeCancel).toBeDefined();

    await act(async () => activeCancel?.click());
    expect(invokeCalls).toContainEqual({ command: "cancel_cache_move", args: {} });
    expect(screen.getByText("Cancelling…")).toBeTruthy();
  });

  it("refuses programmatic close while a save is still committing", () => {
    useSettingsStore.setState({ saving: true });
    useSettingsStore.getState().close();
    expect(useSettingsStore.getState().open).toBe(true);
  });
});
