// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import SettingsModal from "../../src/components/SettingsModal";
import { useSettingsStore } from "../../src/state/settings-store";
import { createdWindows, invokeCalls, mockCommands, resetTauriMocks, setMonitors, setWindowCreatedHook, WebviewWindow } from "../mocks/tauri";

const config = {
  sourceDirs: ["C:\\Photos"],
  defaultTimezone: "Asia/Tokyo",
  aiAcceleration: { transcription: "metal", "face-scoring": "none" },
};

const accelerationCapabilities = [
  {
    feature: "transcription",
    label: "Transcription",
    selected: "metal",
    default: "metal",
    options: [
      { id: "none", label: "CPU only" },
      { id: "metal", label: "Metal" },
    ],
  },
  {
    feature: "face-scoring",
    label: "Face scoring",
    selected: "none",
    default: "none",
    options: [{ id: "none", label: "CPU only" }],
  },
];

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    rebuild_library_index: () => null,
    get_section_counts: () => ({ images: [], videos: [], others: [] }),
    get_issues: () => ({ total: 0, rows: [] }),
    text_encodings: () => ["utf-8", "shift_jis"],
    index_work_snapshot: () => ({
      sourceCheck: {
        running: true,
        stopping: false,
        waiting: false,
        lastResult: "stopped",
        eventSequence: 1,
      },
      fileInformation: {
        running: false,
        paused: false,
        stopping: false,
        queued: false,
        eventSequence: 0,
      },
    }),
  });
  useSettingsStore.getState().beginEditing(config, null, accelerationCapabilities);
});

afterEach(() => {
  setWindowCreatedHook(null);
  useSettingsStore.setState({
    draft: null,
    opened: null,
    saving: false,
  });
  cleanup();
});

describe("Settings categories", () => {
  it("keeps screen controls reachable after identification failure and clears the result on retry", async () => {
    setMonitors([0, 1].map((index) => ({
      name: `Fixture ${index}`, position: { x: index * 1920, y: 0 },
      size: { width: 1920, height: 1080 }, scaleFactor: 1,
    })));
    mockCommands({ record_recent_notification: () => ({ id: 1 }) });
    let fail = true;
    setWindowCreatedHook((label) => {
      queueMicrotask(async () => {
        const window = (await WebviewWindow.getByLabel(label))!;
        const handlers = window.once.mock.calls as unknown as Array<[string, (event: { payload: unknown }) => void]>;
        handlers.find(([event]) => event === (fail ? "tauri://error" : "tauri://created"))![1]({ payload: "fixture native failure" });
      });
    });
    render(<SettingsModal open onClose={() => {}} />);
    fireEvent.click(screen.getByRole("tab", { name: "Appearance" }));
    const identify = await screen.findByRole("button", { name: "Identify screens" });
    await act(async () => identify.click());
    expect(screen.getByText("Couldn’t identify all connected screens. Try again.")).toBeTruthy();
    expect(screen.getAllByRole("button", { name: "Move up" })).toHaveLength(2);
    expect((screen.getByRole("button", { name: "Identify screens" }) as HTMLButtonElement).disabled).toBe(false);
    expect(invokeCalls.filter((call) => call.command === "record_recent_notification")).toHaveLength(1);

    fail = false;
    await act(async () => screen.getByRole("button", { name: "Identify screens" }).click());
    expect(screen.queryByText(/Couldn’t identify/)).toBeNull();
    const oldWindow = (await WebviewWindow.getByLabel(createdWindows[0].label))!;
    const oldHandlers = oldWindow.once.mock.calls as unknown as Array<[string, (event: { payload: unknown }) => void]>;
    await act(async () => oldHandlers.find(([event]) => event === "tauri://error")![1]({ payload: "obsolete failure" }));
    expect(screen.queryByText(/Couldn’t identify/)).toBeNull();
    expect(invokeCalls.filter((call) => call.command === "record_recent_notification")).toHaveLength(1);
  });

  it("explains how to populate an empty source-directory list", () => {
    useSettingsStore.getState().beginEditing({ ...config, sourceDirs: [] });
    render(<SettingsModal open onClose={() => {}} />);

    expect(screen.getByText(/No source directories/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add directory" })).toBeTruthy();
  });

  it("uses keyboard-operable tabs instead of one long mixed scroller", () => {
    render(<SettingsModal open onClose={() => {}} />);
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
    useSettingsStore.getState().beginEditing({
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
    render(<SettingsModal open onClose={() => {}} />);

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
    render(<SettingsModal open onClose={() => {}} />);

    fireEvent.click(screen.getByRole("button", { name: /Rebuild library index/ }));
    expect(screen.getByText(/Your files, settings, managed tools, and choices/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Rebuild" }));

    await waitFor(() =>
      expect(invokeCalls.some((call) => call.command === "rebuild_library_index")).toBe(true),
    );
  });

  it("keeps video and audio policy separate and face scoring before trash behavior", () => {
    render(<SettingsModal open onClose={() => {}} />);
    fireEvent.click(screen.getByRole("tab", { name: "Media" }));
    expect(screen.getByLabelText("Play videos automatically when shown")).toBeTruthy();
    expect(screen.getByLabelText("Play audio automatically when shown")).toBeTruthy();
    expect(screen.queryByLabelText("Play after choosing a snapshot")).toBeNull();

    fireEvent.click(screen.getByRole("tab", { name: "Behavior" }));
    expect(screen.getByLabelText("Sound")).toBeTruthy();
    const labels = Array.from(screen.getByRole("tabpanel").querySelectorAll("label")).map(
      (label) => label.textContent,
    );
    const labelIndex = (prefix: string) =>
      labels.findIndex((label) => label?.startsWith(prefix));
    expect(labelIndex("Score faces for photo ordering")).toBeLessThan(
      labelIndex("Show face-score stars on photos"),
    );
    expect(labelIndex("Show face-score stars on photos")).toBeLessThan(
      labelIndex("Maximum images in Comparison"),
    );
    expect(labelIndex("Maximum images in Comparison")).toBeLessThan(
      labelIndex("Confirm direct single-item deletion"),
    );
    expect(
      (screen.getByLabelText("Show face-score stars on photos") as HTMLInputElement).checked,
    ).toBe(true);
    expect(
      (screen.getByLabelText(/Maximum images in Comparison/) as HTMLInputElement).value,
    ).toBe("16");
  });

  it("renders backend-owned acceleration choices and switches Metal at runtime", () => {
    render(<SettingsModal open onClose={() => {}} />);
    fireEvent.click(screen.getByRole("tab", { name: "Behavior" }));

    const transcription = screen.getByLabelText("Transcription acceleration") as HTMLSelectElement;
    expect(transcription.value).toBe("metal");
    fireEvent.change(transcription, { target: { value: "none" } });
    expect(useSettingsStore.getState().draft?.aiAcceleration).toEqual({
      transcription: "none",
      "face-scoring": "none",
    });
    expect(screen.getByText("CPU only", { selector: "span" })).toBeTruthy();
  });
});

describe("settings save state", () => {
  it("refuses to discard the draft while a save is still committing", () => {
    useSettingsStore.setState({ saving: true });
    useSettingsStore.getState().discardDraft();
    expect(useSettingsStore.getState().draft).not.toBeNull();
  });
});
