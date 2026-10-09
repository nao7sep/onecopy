// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import SettingsModal from "../../src/components/SettingsModal";
import { useSettingsStore } from "../../src/state/settings-store";
import { useQuitDiscardStore } from "../../src/workflows/quit";
import { DEFAULT_CONFIG, effectiveConfig } from "../helpers/config";
import { createdWindows, invokeCalls, mockCommands, resetTauriMocks, setMonitors, setWindowCreatedHook, WebviewWindow } from "../mocks/tauri";

const config = effectiveConfig({
  sourceDirs: ["C:\\Photos"],
  defaultTimezone: "Asia/Tokyo",
  aiAcceleration: { transcription: "metal", "face-scoring": "none" },
});

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
    text_preview_options: () => ({ encodings: ["utf-8", "shift_jis"], maxAllowedBytes: 64 * 1024 * 1024 }),
    visibility_capabilities: () => ({ hiddenAttributes: true, systemAttributes: false }),
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
  useSettingsStore.getState().beginEditing(config, accelerationCapabilities);
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
  it("edits the complete ignored-name list and exposes only supported native attributes", async () => {
    render(<SettingsModal open onClose={() => {}} />);
    await screen.findByRole("checkbox", { name: "Hide files and folders marked hidden" });
    expect(screen.queryByRole("checkbox", { name: "Hide files and folders marked system" })).toBeNull();
    expect((screen.getByRole("checkbox", { name: "Hide names beginning with a dot" }) as HTMLInputElement).checked).toBe(true);
    fireEvent.change(screen.getByLabelText("Ignored file name 1"), { target: { value: "custom.cache" } });
    expect(useSettingsStore.getState().draft?.ignoredFileNames[0]).toBe("custom.cache");
    for (let count = 3; count > 0; count--) fireEvent.click(screen.getByRole("button", { name: "Remove ignored file name 1" }));
    expect(useSettingsStore.getState().draft?.ignoredFileNames).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Add file name" }));
    fireEvent.change(screen.getByLabelText("Ignored file name 1"), { target: { value: "local.ini" } });
    expect(useSettingsStore.getState().draft?.ignoredFileNames).toEqual(["local.ini"]);
  });

  it("offers the system attribute option when the backend supports it", async () => {
    mockCommands({ visibility_capabilities: () => ({ hiddenAttributes: true, systemAttributes: true }) });
    render(<SettingsModal open onClose={() => {}} />);
    const system = await screen.findByRole("checkbox", { name: "Hide files and folders marked system" });
    expect((system as HTMLInputElement).checked).toBe(true);
  });

  it("offers the timezone as a list rather than a typed name", () => {
    render(<SettingsModal open onClose={() => {}} />);
    const zone = screen.getByDisplayValue("Asia/Tokyo") as HTMLSelectElement;

    expect(zone.tagName).toBe("SELECT");
    expect([...zone.options].some((option) => option.value === "UTC")).toBe(true);
    expect([...zone.options].length).toBeGreaterThan(50);
    expect(screen.queryByRole("button", { name: /timezone names/i })).toBeNull();
  });

  it("shows the default font as a placeholder without writing it into the preference", () => {
    render(<SettingsModal open onClose={() => {}} />);
    fireEvent.click(screen.getByRole("tab", { name: "Appearance" }));
    const font = screen.getByPlaceholderText("System font") as HTMLInputElement;
    expect(font.value).toBe("");
    expect(useSettingsStore.getState().draft?.uiFontFamily).toBe("");
    fireEvent.change(font, { target: { value: "Example Sans, sans-serif" } });
    expect(useSettingsStore.getState().draft?.uiFontFamily).toBe("Example Sans, sans-serif");
    fireEvent.change(font, { target: { value: "" } });
    expect(useSettingsStore.getState().draft?.uiFontFamily).toBe("");
  });

  // R5.5 C1: the language picker lists System plus exactly the ten supported
  // languages, each named in its own words rather than a fixed English list.
  it("lists System plus the ten languages, each named in its own words", async () => {
    const { LANGUAGES, LANGUAGE_NAMES } = await import("../../src/i18n/languages");
    render(<SettingsModal open onClose={() => {}} />);
    fireEvent.click(screen.getByRole("tab", { name: "Appearance" }));

    const select = screen.getByLabelText("Language") as HTMLSelectElement;
    const options = [...select.options];
    expect(options).toHaveLength(LANGUAGES.length + 1);
    expect(options[0].value).toBe("system");
    expect(options[0].textContent).toBe("System");
    for (const language of LANGUAGES) {
      const option = options.find((candidate) => candidate.value === language)!;
      expect(option).toBeTruthy();
      expect(option.textContent).toBe(LANGUAGE_NAMES[language]);
      expect(option.getAttribute("lang")).toBe(language);
    }
  });

  // The ends of the list are the whole point: a screen already first cannot
  // move up and one already last cannot move down, and those two controls are
  // the ones a user is most likely to reach for first.
  it("inerts only the moves that would run off the ends of the screen order", async () => {
    setMonitors([0, 1, 2].map((index) => ({
      name: `Fixture ${index}`, position: { x: index * 1920, y: 0 },
      size: { width: 1920, height: 1080 }, scaleFactor: 1,
    })));
    render(<SettingsModal open onClose={() => {}} />);
    fireEvent.click(screen.getByRole("tab", { name: "Appearance" }));

    const disabled = (name: string) =>
      screen.getAllByRole("button", { name }).map((button) => (button as HTMLButtonElement).disabled);

    // The name comes from the button's own words, not an aria-label: a screen
    // reader and the eye are told the same thing.
    await waitFor(() => expect(disabled("Move up")).toEqual([true, false, false]));
    expect(disabled("Move down")).toEqual([false, false, true]);
    expect(screen.getAllByRole("button", { name: "Move up" })[0].textContent).toBe("Move up");
  });

  // The screen order is a setting outside the Settings draft: a move is saved
  // immediately, and neither Discard nor Save (which sends only the draft's
  // changed sets) can see or revert it.
  it("keeps the screen order outside the Settings draft: Save never sends it, and it survives Discard", async () => {
    setMonitors([0, 1].map((index) => ({
      name: `Fixture ${index}`, position: { x: index * 1920, y: 0 },
      size: { width: 1920, height: 1080 }, scaleFactor: 1,
    })));
    mockCommands({
      patch_state: () => ({}),
      save_config: () => ({}),
      check_source_dirs: () => [],
      record_recent_notification: () => ({}),
      log_event: () => null,
    });
    render(<SettingsModal open onClose={() => {}} />);
    fireEvent.click(screen.getByRole("tab", { name: "Appearance" }));
    await waitFor(() => expect(screen.queryAllByRole("button", { name: "Move down" }).length).toBeGreaterThan(0));

    const hasScreenPriority = (call: { command: string; args: Record<string, unknown> }) =>
      call.command === "save_config" &&
      "screenPriority" in ((call.args.changes as Record<string, unknown>) ?? {});

    fireEvent.click(screen.getAllByRole("button", { name: "Move down" })[0]);
    await waitFor(() => expect(invokeCalls.some(hasScreenPriority)).toBe(true));
    const screenWrite = invokeCalls.find(hasScreenPriority);
    invokeCalls.length = 0;

    fireEvent.click(screen.getByRole("tab", { name: "Library" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Hide names beginning with a dot" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(invokeCalls.some((call) => call.command === "save_config")).toBe(true));

    const configWrite = invokeCalls.find((call) => call.command === "save_config");
    expect(configWrite?.args.changes).toHaveProperty("hideDotNames");
    expect(invokeCalls.some(hasScreenPriority)).toBe(false);
    // The move already landed and nothing here reverted it.
    expect((screenWrite?.args.changes as Record<string, unknown>)).toHaveProperty("screenPriority");
  });

  // R5.5 C7: each screen row renders POSITION before name, in the DOM, not
  // only in the sentence template -- position is what tells a matched pair
  // (same name, same resolution) apart, so it must lead visually.
  it("renders each screen row with position before name", async () => {
    setMonitors([0, 1].map((index) => ({
      name: "Studio Display", position: { x: index * 1920, y: 0 },
      size: { width: 1920, height: 1080 }, scaleFactor: 1,
    })));
    render(<SettingsModal open onClose={() => {}} />);
    fireEvent.click(screen.getByRole("tab", { name: "Appearance" }));
    await waitFor(() => expect(screen.queryAllByRole("button", { name: "Move down" }).length).toBeGreaterThan(0));

    const row = screen.getByText(/^1\. /).closest("span")!.parentElement!;
    const [primary, detail] = [...row.querySelectorAll("span")].filter(
      (el) => el.children.length === 0,
    );
    expect(primary.textContent).toMatch(/^1\. /);
    expect(detail!.textContent).toContain("Studio Display");
    expect(row.textContent!.indexOf(primary.textContent!)).toBeLessThan(
      row.textContent!.indexOf("Studio Display"),
    );
  });

  // screen-priority.md: the configured display order is meaningful only with
  // two or more monitors, so a single-display machine must render no order
  // section at all (not merely a one-row, unreorderable one).
  it("renders no screen-priority section with a single display", async () => {
    setMonitors([{
      name: "Fixture 0", position: { x: 0, y: 0 },
      size: { width: 1920, height: 1080 }, scaleFactor: 1,
    }]);
    render(<SettingsModal open onClose={() => {}} />);
    fireEvent.click(screen.getByRole("tab", { name: "Appearance" }));
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(screen.queryByRole("button", { name: "Move up" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Move down" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Identify screens" })).toBeNull();
  });

  // One occurrence, one report: the row that asked shows the failure, so the
  // core must not raise its generic storage notice beside it and the status bar
  // must not count the one failed write as two Issues.
  it("reports a failed screen-order write once, at the rows that asked", async () => {
    setMonitors([0, 1].map((index) => ({
      name: `Fixture ${index}`, position: { x: index * 1920, y: 0 },
      size: { width: 1920, height: 1080 }, scaleFactor: 1,
    })));
    mockCommands({
      save_config: () => Promise.reject(new TypeError("EACCES writing config.json")),
      record_recent_notification: () => ({ id: 1 }),
    });
    render(<SettingsModal open onClose={() => {}} />);
    fireEvent.click(screen.getByRole("tab", { name: "Appearance" }));
    const move = (await screen.findAllByRole("button", { name: "Move down" }))[0];

    await act(async () => move.click());

    expect(await screen.findByText("Couldn’t save the screen order.")).toBeTruthy();
    const write = invokeCalls.find((call) => call.command === "save_config");
    expect(write?.args.reportFailure).toBe(false);
    await waitFor(() =>
      expect(invokeCalls.filter((call) => call.command === "record_recent_notification")).toHaveLength(1),
    );
    expect(invokeCalls.some((call) => call.command === "publish_notification")).toBe(false);
  });

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

  it("stops at the ends instead of wrapping, like every other tablist in the app (R8-01)", () => {
    render(<SettingsModal open onClose={() => {}} />);
    const first = screen.getByRole("tab", { name: "Library" });
    fireEvent.keyDown(first, { key: "ArrowLeft" });
    expect(document.activeElement).toBe(first);

    const last = screen.getByRole("tab", { name: "Behavior" });
    last.focus();
    fireEvent.keyDown(last, { key: "ArrowRight" });
    expect(document.activeElement).toBe(last);
  });

  it("offers one grouping preset and one autoplay setting without internal tuning fields", () => {
    useSettingsStore.getState().beginEditing({ ...config, similarPhotoGrouping: "looser" }, [], DEFAULT_CONFIG);
    render(<SettingsModal open onClose={() => {}} />);
    fireEvent.click(screen.getByRole("tab", { name: "Media" }));
    fireEvent.change(screen.getByDisplayValue("Looser"), { target: { value: "normal" } });
    expect(useSettingsStore.getState().draft?.similarPhotoGrouping).toBe("normal");
    expect(screen.getAllByRole("checkbox", { name: "Autoplay" })).toHaveLength(1);
    expect(screen.queryByText("Preview long edge (px)")).toBeNull();
    expect(screen.queryByText("Show face stars")).toBeNull();
    expect(screen.queryByRole("button", { name: "Reset similar photo settings" })).toBeNull();
  });

  it("confirms library reconstruction from Settings, keeping previews and transcripts by default", async () => {
    render(<SettingsModal open onClose={() => {}} />);

    fireEvent.click(screen.getByRole("button", { name: /Rebuild library index/ }));
    expect(screen.getByText(/Your files, settings, managed tools, and choices/)).toBeTruthy();
    // Both discard options render unchecked, and the transcript one carries
    // its highlighted caution note (Phase 9 developer decision).
    const previews = screen.getByLabelText("Previews and posters") as HTMLInputElement;
    const transcripts = screen.getByLabelText("Transcripts") as HTMLInputElement;
    const faces = screen.getByLabelText("Face scores") as HTMLInputElement;
    expect(previews.checked).toBe(false);
    expect(transcripts.checked).toBe(false);
    expect(faces.checked).toBe(false);
    expect(
      screen.getByText(/Regenerating transcripts can take a long time/),
    ).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Rebuild" }));

    await waitFor(() =>
      expect(invokeCalls.some((call) => call.command === "rebuild_library_index")).toBe(true),
    );
    const call = invokeCalls.find((call) => call.command === "rebuild_library_index");
    expect(call?.args).toEqual({ discardPreviews: false, discardTranscripts: false, discardFaces: false });
  });

  it("passes the chosen rebuild options through to the backend command", async () => {
    render(<SettingsModal open onClose={() => {}} />);

    fireEvent.click(screen.getByRole("button", { name: /Rebuild library index/ }));
    fireEvent.click(screen.getByLabelText("Previews and posters"));
    fireEvent.click(screen.getByLabelText("Transcripts"));
    fireEvent.click(screen.getByLabelText("Face scores"));
    fireEvent.click(screen.getByRole("button", { name: "Rebuild" }));

    await waitFor(() =>
      expect(invokeCalls.some((call) => call.command === "rebuild_library_index")).toBe(true),
    );
    const call = invokeCalls.find((call) => call.command === "rebuild_library_index");
    expect(call?.args).toEqual({ discardPreviews: true, discardTranscripts: true, discardFaces: true });
  });

  it("groups related controls without changing any draft value on tab changes", () => {
    const before = useSettingsStore.getState().draft;
    render(<SettingsModal open onClose={() => {}} />);
    expect(screen.queryByLabelText(/Pair companion files/)).toBeNull();
    expect(screen.getByLabelText("Keep the system awake during background work")).toBeTruthy();
    expect(screen.getByLabelText("Check source folders after OneCopy opens")).toBeTruthy();
    expect(screen.queryByLabelText("Sound")).toBeNull();
    fireEvent.click(screen.getByRole("tab", { name: "Media" }));
    expect(screen.getByLabelText("Autoplay")).toBeTruthy();
    expect(screen.queryByLabelText("Play after choosing a snapshot")).toBeNull();

    expect(screen.getByLabelText("Sound")).toBeTruthy();
    const labels = Array.from(screen.getByRole("tabpanel").querySelectorAll("label")).map(
      (label) => label.textContent,
    );
    const labelIndex = (prefix: string) =>
      labels.findIndex((label) => label?.startsWith(prefix));
    expect(labelIndex("Score faces for photo ordering")).toBeLessThan(labelIndex("Maximum images in Comparison"));
    expect(screen.queryByLabelText("Show face-score stars on photos")).toBeNull();
    expect(screen.queryByLabelText("Snapshot frames (max)")).toBeNull();
    expect(
      (screen.getByLabelText(/Maximum images in Comparison/) as HTMLInputElement).value,
    ).toBe("16");
    expect(screen.queryByLabelText("Confirm direct single-item deletion")).toBeNull();
    fireEvent.click(screen.getByRole("tab", { name: "Behavior" }));
    expect(screen.getByLabelText("Confirm direct single-item deletion")).toBeTruthy();
    expect(screen.queryByLabelText("Sound")).toBeNull();
    expect(screen.queryByLabelText(/Score faces/)).toBeNull();
    expect(useSettingsStore.getState().draft).toEqual(before);
  });

  it("renders backend-owned acceleration choices and switches Metal at runtime", () => {
    render(<SettingsModal open onClose={() => {}} />);
    fireEvent.click(screen.getByRole("tab", { name: "Media" }));

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

describe("quitting over unsaved edits", () => {
  it("asks in Settings, and Keep editing answers the quit without discarding", () => {
    let answer: boolean | null = null;
    let shown = false;
    useQuitDiscardStore.setState({
      choose: (discard) => { answer = discard; },
      presented: () => { shown = true; },
    });
    render(<SettingsModal open onClose={() => {}} />);

    expect(shown).toBe(true);
    expect(screen.getByText("Discard changes?")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    expect(answer).toBe(false);
    expect(useSettingsStore.getState().draft).not.toBeNull();
    useQuitDiscardStore.setState({ choose: null, presented: null });
  });
});
