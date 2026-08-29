// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
    rebuild_library_index: () => null,
    get_section_counts: () => ({ images: [], videos: [], others: [] }),
    get_issues: () => ({ total: 0, rows: [] }),
    index_work_snapshot: () => ({
      sourceCheck: { running: true, stopping: false },
      fileInformation: { running: false, paused: false, stopping: false, queued: false },
    }),
  });
  useSettingsStore.getState().openWith(config);
});

afterEach(() => {
  useSettingsStore.setState({
    open: false,
    draft: null,
    opened: null,
    saving: false,
  });
  cleanup();
});

describe("Settings categories", () => {
  it("explains how to populate an empty source-directory list", () => {
    useSettingsStore.getState().openWith({ ...config, sourceDirs: [] });
    render(<SettingsModal />);

    expect(screen.getByText(/No source directories/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add directory" })).toBeTruthy();
  });

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

  it("resets only the four optimized similar-photo settings", () => {
    useSettingsStore.getState().openWith({
      ...config,
      goodRangeStartYear: 2007,
      similarityMaxGapSeconds: 12,
      similarityPhashMaxDistance: 19,
      similarityPhashMaxDistanceBurst: 27,
      similarityDiameterMultiplier: 4,
      previewLongEdgePx: 2048,
      confirmTrashDelete: true,
    });
    const before = useSettingsStore.getState().draft;
    render(<SettingsModal />);

    fireEvent.click(screen.getByRole("button", { name: "Reset similar photo settings" }));

    expect(useSettingsStore.getState().draft).toEqual({
      ...before,
      similarityMaxGapSeconds: 90,
      similarityPhashMaxDistance: 3,
      similarityPhashMaxDistanceBurst: 10,
      similarityDiameterMultiplier: 2,
    });
  });

  it("confirms library reconstruction from Settings", async () => {
    render(<SettingsModal />);

    fireEvent.click(screen.getByRole("button", { name: /Rebuild library index/ }));
    expect(screen.getByText(/Your files, settings, managed tools, and choices/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Rebuild" }));

    await waitFor(() =>
      expect(invokeCalls.some((call) => call.command === "rebuild_library_index")).toBe(true),
    );
  });

  it("puts video playback choices together and face scoring before trash behavior", () => {
    render(<SettingsModal />);
    fireEvent.click(screen.getByRole("tab", { name: "Media" }));
    expect(screen.getByLabelText("Play videos automatically when shown")).toBeTruthy();
    expect(screen.getByLabelText("Play after choosing a snapshot")).toBeTruthy();

    fireEvent.click(screen.getByRole("tab", { name: "Behavior" }));
    const labels = Array.from(screen.getByRole("tabpanel").querySelectorAll("label")).map(
      (label) => label.textContent,
    );
    expect(labels.indexOf("Score faces for photo ordering (background, needs the face models)"))
      .toBeLessThan(labels.indexOf("Show face-score stars on photos"));
    expect(labels.indexOf("Show face-score stars on photos")).toBeLessThan(
      labels.indexOf("Confirm before moving items to trash"),
    );
    expect(
      (screen.getByLabelText("Show face-score stars on photos") as HTMLInputElement).checked,
    ).toBe(true);
  });
});

describe("settings save state", () => {
  it("refuses programmatic close while a save is still committing", () => {
    useSettingsStore.setState({ saving: true });
    useSettingsStore.getState().close();
    expect(useSettingsStore.getState().open).toBe(true);
  });
});
