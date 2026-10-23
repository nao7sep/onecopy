import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";
import { flushConfigForShutdown, resetConfigWritesForTests, useAppStore, retainStatePatch } from "../../src/state/app-store";
import { setSoundEnabled, setPlaybackVolume } from "../../src/workflows/playback";
import { cancelQuitDecision, cancelSessionEnd, endSession, installQuitWorkflow, requestQuit, useQuitDiscardStore, useQuitRunningWorkStore, useQuitSaveStore } from "../../src/workflows/quit";
import { useMutationStore } from "../../src/state/mutation-store";
import { useBinariesStore, type DependencyState } from "../../src/state/binaries-store";
import { useDerivedWorkStore } from "../../src/state/derived-work-store";
import { useSectionsStore } from "../../src/state/sections-store";
import type { MutationKind } from "../../src/models/mutation";
import { createTranslator } from "../../src/i18n/translate";
import { useFullscreenViewStore } from "../../src/state/fullscreen-view-store";
import { closeFullscreenView } from "../../src/workflows/fullscreen-view";

// The fullscreen view's own close path is tested with the view; here it only
// has to be the one quit takes.
vi.mock("../../src/workflows/fullscreen-view", () => ({ closeFullscreenView: vi.fn() }));
import { useSettingsStore } from "../../src/state/settings-store";
import { useAppShellStore } from "../../src/state/app-shell-store";
import { effectiveConfig } from "../helpers/config";
import { fireEvent, invokeCalls, mockCommands, resetTauriMocks, setCurrentWindowShown } from "../mocks/tauri";

// The native events reach listeners installed once, as at launch.
beforeAll(installQuitWorkflow);
beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useDerivedWorkStore.setState({ runtime: null, snapshot: null, activeItem: null });
  resetConfigWritesForTests();
  cancelSessionEnd();
  useSettingsStore.setState({ draft: null, opened: null, saving: false });
  useAppShellStore.getState().closeUtility();
  useMutationStore.setState({ progress: null, cancelling: false });
  useBinariesStore.setState({ entries: [], installing: {} });
  useQuitRunningWorkStore.setState({ body: null });
  useAppStore.setState({ appData: { config: { soundEnabled: true, playbackVolume: 0.7 }, state: {}, dataRoot: "/app", debugEnabled: false, quarantines: [] } });
  mockCommands({
    save_config: ({ changes }) => ({ ...useAppStore.getState().appData!.config, ...(changes as object) }),
    patch_state: ({ patch }) => patch,
    request_app_exit: () => null,
    session_end_saved: () => null,
    log_event: () => null,
    record_interface_failure: () => null,
  });
});
afterEach(() => { cancelSessionEnd(); vi.useRealTimers(); });
async function settle() { for (let i = 0; i < 20; i++) await Promise.resolve(); }
function exits() { return invokeCalls.filter((call) => call.command === "request_app_exit"); }

it("native requests join one save and exit despite an optional state failure", async () => {
  setPlaybackVolume(0.4);
  retainStatePatch({ zoom: 1.2 });
  mockCommands({ patch_state: () => Promise.reject(new Error("layout cannot be saved")) });
  fireEvent("app://quit-request", null);
  const joined = requestQuit();
  expect(requestQuit()).toBe(joined);
  await joined;
  expect(invokeCalls.filter((call) => call.command === "save_config")).toHaveLength(1);
  expect(exits()).toHaveLength(1);
  expect(useQuitSaveStore.getState().choose).toBeNull();
});

it("native session-end events save, report that save by id and, once cancelled, let quit work again", async () => {
  setPlaybackVolume(0.4);
  fireEvent("app://session-ending", 7);
  await settle();
  expect(invokeCalls.find((call) => call.command === "session_end_saved")?.args).toEqual({ id: 7 });
  expect(invokeCalls.filter((call) => call.command === "save_config")).toHaveLength(1);
  await requestQuit();
  expect(exits()).toHaveLength(0);
  fireEvent("app://session-end-cancelled", null);
  await requestQuit();
  expect(exits()).toHaveLength(1);
});

it("failed required save cancels exit and Retry saves later edits", async () => {
  setPlaybackVolume(0.3);
  mockCommands({ save_config: () => Promise.reject(new Error("disk full")) });
  const quit = requestQuit();
  await settle();
  expect(exits()).toHaveLength(0);
  expect(useQuitSaveStore.getState().choose).not.toBeNull();
  setPlaybackVolume(0.8);
  mockCommands({ save_config: ({ changes }) => ({ ...useAppStore.getState().appData!.config, ...(changes as object) }) });
  useQuitSaveStore.getState().choose!("retry");
  await quit;
  expect(exits()).toHaveLength(1);
  expect(useAppStore.getState().appData?.config.playbackVolume).toBe(0.8);
});

it("Cancel retains failed changes for a later quit", async () => {
  setPlaybackVolume(0.3);
  mockCommands({ save_config: () => Promise.reject(new Error("disk full")) });
  const quit = requestQuit();
  await settle();
  useQuitSaveStore.getState().choose!("cancel");
  await quit;
  expect(exits()).toHaveLength(0);
  mockCommands({ save_config: ({ changes }) => ({ ...useAppStore.getState().appData!.config, ...(changes as object) }) });
  await requestQuit();
  expect(exits()).toHaveLength(1);
  expect(useAppStore.getState().appData?.config.playbackVolume).toBe(0.3);
});

it("only explicit Quit Anyway authorizes exit after required save failure", async () => {
  setPlaybackVolume(0.4);
  mockCommands({ save_config: () => Promise.reject(new Error("disk full")) });
  const quit = requestQuit();
  await settle();
  useQuitSaveStore.getState().choose!("quit");
  await quit;
  expect(exits()).toHaveLength(1);
  mockCommands({ save_config: ({ changes }) => changes });
  await flushConfigForShutdown();
});

it("OS takeover settles a pending question and reports saving without ordinary quit consent", async () => {
  setPlaybackVolume(0.4);
  mockCommands({ save_config: () => Promise.reject(new Error("disk full")) });
  const quit = requestQuit();
  await settle();
  expect(useQuitSaveStore.getState().choose).not.toBeNull();
  await endSession();
  await quit;
  expect(useQuitSaveStore.getState().choose).toBeNull();
  expect(exits()).toHaveLength(0);
  expect(invokeCalls.filter((call) => call.command === "session_end_saved")).toHaveLength(1);
  mockCommands({ save_config: ({ changes }) => changes });
  await flushConfigForShutdown();
});

it("a stalled required save times out to Cancel while its late write remains owned", async () => {
  vi.useFakeTimers();
  let complete!: (value: unknown) => void;
  setPlaybackVolume(0.4);
  mockCommands({ save_config: () => new Promise((resolve) => { complete = resolve; }) });
  const quit = requestQuit();
  await settle();
  await vi.advanceTimersByTimeAsync(1500);
  expect(exits()).toHaveLength(0);
  useQuitSaveStore.getState().choose!("cancel");
  await quit;
  complete({ soundEnabled: true, playbackVolume: 0.4 });
  await settle();
  await flushConfigForShutdown();
});

it("a cancelled OS takeover cannot resurrect the interrupted ordinary quit", async () => {
  let complete!: (value: unknown) => void;
  setPlaybackVolume(0.4);
  mockCommands({ save_config: () => new Promise((resolve) => { complete = resolve; }) });
  const quit = requestQuit();
  await settle();
  const session = endSession(12);
  cancelSessionEnd();
  complete({ soundEnabled: true, playbackVolume: 0.4 });
  await session;
  await quit;
  expect(exits()).toHaveLength(0);
  expect(invokeCalls.find((call) => call.command === "session_end_saved")?.args).toEqual({ id: 12 });
});

it("quit Retry saves the retained visible Sound choice", async () => {
  mockCommands({ save_config: () => Promise.reject(new Error("disk full")) });
  await expect(setSoundEnabled(false)).rejects.toThrow("disk full");
  expect(useAppStore.getState().appData?.config.soundEnabled).toBe(false);
  const quit = requestQuit();
  await settle();
  expect(exits()).toHaveLength(0);
  mockCommands({ save_config: ({ changes }) => ({ ...useAppStore.getState().appData!.config, ...(changes as object) }) });
  useQuitSaveStore.getState().choose!("retry");
  await quit;
  expect(exits()).toHaveLength(1);
  expect(useAppStore.getState().appData?.config.soundEnabled).toBe(false);
});

it("missing required question presentation cancels quit and retains the failed patch", async () => {
  vi.useFakeTimers();
  setPlaybackVolume(0.4);
  mockCommands({ save_config: () => Promise.reject(new Error("disk full")) });
  const quit = requestQuit();
  await settle();
  expect(useQuitSaveStore.getState().choose).not.toBeNull();
  await vi.advanceTimersByTimeAsync(500);
  await quit;
  expect(useQuitSaveStore.getState().choose).toBeNull();
  expect(exits()).toHaveLength(0);
  mockCommands({ save_config: ({ changes }) => changes });
  await flushConfigForShutdown();
});

it("renderer failure cancels a question that was already presented", async () => {
  vi.useFakeTimers();
  setPlaybackVolume(0.4);
  mockCommands({ save_config: () => Promise.reject(new Error("disk full")) });
  const quit = requestQuit();
  await settle();
  useQuitSaveStore.getState().presented!();
  await settle();
  expect(useQuitSaveStore.getState().choose).not.toBeNull();
  cancelQuitDecision();
  await quit;
  expect(exits()).toHaveLength(0);
  expect(useQuitSaveStore.getState().choose).toBeNull();
  mockCommands({ save_config: ({ changes }) => changes });
  await flushConfigForShutdown();
});

// Unsaved Settings edits and an ordinary quit: ask, and
// never on the OS path.
function editSettings(): void {
  useSettingsStore.getState().beginEditing(effectiveConfig());
  useAppShellStore.getState().openUtility("settings");
  useSettingsStore.getState().update({ defaultTimezone: "Asia/Tokyo" });
}

async function presentedDiscardQuestion(): Promise<(discard: boolean) => void> {
  await settle();
  useQuitDiscardStore.getState().presented!();
  const choose = useQuitDiscardStore.getState().choose;
  expect(choose).not.toBeNull();
  return choose!;
}

it("asks before quitting over unsaved Settings edits, and Keep editing stays", async () => {
  editSettings();
  const quit = requestQuit();
  const choose = await presentedDiscardQuestion();
  choose(false);
  await quit;
  expect(exits()).toHaveLength(0);
  expect(useSettingsStore.getState().draft?.defaultTimezone).toBe("Asia/Tokyo");
  expect(useAppShellStore.getState().utilitySurface).toBe("settings");
  expect(useQuitDiscardStore.getState().choose).toBeNull();
});

it("Discard drops the unsaved Settings edits and quits without saving them", async () => {
  editSettings();
  const quit = requestQuit();
  (await presentedDiscardQuestion())(true);
  await quit;
  expect(exits()).toHaveLength(1);
  expect(useSettingsStore.getState().draft).toBeNull();
  expect(useAppShellStore.getState().utilitySurface).toBeNull();
  const written = invokeCalls.filter((call) => call.command === "save_config")
    .map((call) => call.args.changes as Record<string, unknown>);
  expect(written.some((changes) => "defaultTimezone" in changes)).toBe(false);
});

it("quits without asking when Settings is closed or has no edits", async () => {
  useSettingsStore.getState().beginEditing(effectiveConfig());
  useAppShellStore.getState().openUtility("settings");
  await requestQuit();
  expect(exits()).toHaveLength(1);
  expect(useQuitDiscardStore.getState().choose).toBeNull();
});

it("a question that cannot be shown keeps editing", async () => {
  vi.useFakeTimers();
  editSettings();
  const quit = requestQuit();
  await settle();
  await vi.advanceTimersByTimeAsync(500);
  await quit;
  expect(exits()).toHaveLength(0);
  expect(useSettingsStore.getState().draft).not.toBeNull();
});

it("the OS ending the session never asks about unsaved Settings edits", async () => {
  editSettings();
  setPlaybackVolume(0.4);
  await endSession(3);
  expect(useQuitDiscardStore.getState().choose).toBeNull();
  expect(invokeCalls.filter((call) => call.command === "save_config")
    .map((call) => call.args.changes)).toEqual([{ soundEnabled: true, playbackVolume: 0.4 }]);
  expect(invokeCalls.find((call) => call.command === "session_end_saved")?.args).toEqual({ id: 3 });
});

it("an OS takeover during the question ends the ordinary quit without discarding", async () => {
  editSettings();
  const quit = requestQuit();
  await presentedDiscardQuestion();
  await endSession();
  await quit;
  expect(exits()).toHaveLength(0);
  expect(useSettingsStore.getState().draft).not.toBeNull();
});

it("quit waits for a Settings save in flight and does not ask about it", async () => {
  let complete!: (value: unknown) => void;
  mockCommands({ save_config: ({ changes }) => new Promise((resolve) => {
    complete = () => resolve({ ...useAppStore.getState().appData!.config, ...(changes as object) });
  }) });
  editSettings();
  useSettingsStore.setState({ saving: true });
  const saving = useAppStore.getState().saveConfig({ defaultTimezone: "Asia/Tokyo" });
  const quit = requestQuit();
  await settle();
  expect(exits()).toHaveLength(0);
  expect(useQuitDiscardStore.getState().choose).toBeNull();
  complete(null);
  await saving;
  await quit;
  expect(exits()).toHaveLength(1);
  expect(useAppStore.getState().appData?.config.defaultTimezone).toBe("Asia/Tokyo");
});

// Running work the user started: a file operation or a
// managed-tool install asks before quit stops it; the OS path never asks.
function runOperation(kind: MutationKind = "destination-copy"): void {
  useMutationStore.setState({ cancelling: false, progress: {
    operationId: 1, kind, phase: "delivering", itemsDone: 1, itemsTotal: 4, filesDone: 1, filesTotal: 4,
    bytesDone: 10, bytesTotal: 40, failures: 0, currentFileBytesDone: null, currentFileBytesTotal: null, nextPhase: null,
  } });
}

function downloadFfmpeg(): void {
  useBinariesStore.setState({
    entries: [{ id: "ffmpeg", label: "ffmpeg" } as DependencyState],
    installing: { ffmpeg: { operationId: "install-1", cancelling: false, progress: { phase: "download", done: 1, total: 10, nextPhase: null } } },
  });
}

const english = createTranslator("en");
function runningBody(): string {
  return english.text(useQuitRunningWorkStore.getState().body!);
}

async function presentedRunningWorkQuestion(): Promise<(stop: boolean) => void> {
  await settle();
  const question = useQuitRunningWorkStore.getState();
  expect(question.standalone).toBe(false);
  question.presented!();
  expect(question.choose).not.toBeNull();
  return question.choose!;
}

it("asks before stopping a running file operation, and Keep running cancels the quit", async () => {
  runOperation("destination-move");
  const quit = requestQuit();
  const choose = await presentedRunningWorkQuestion();
  expect(runningBody()).toBe("A move is running.");
  choose(false);
  await quit;
  expect(exits()).toHaveLength(0);
  expect(useQuitRunningWorkStore.getState().choose).toBeNull();
  expect(invokeCalls.some((call) => call.command === "quit_question")).toBe(false);
});

it("Stop and quit exits, and so does an operation that finished while asking", async () => {
  runOperation();
  let quit = requestQuit();
  (await presentedRunningWorkQuestion())(true);
  await quit;
  expect(exits()).toHaveLength(1);

  quit = requestQuit();
  const choose = await presentedRunningWorkQuestion();
  useMutationStore.setState({ progress: null });
  choose(true);
  await quit;
  expect(exits()).toHaveLength(2);
});

it("a repeated quit joins the open question", async () => {
  runOperation();
  const quit = requestQuit();
  const choose = await presentedRunningWorkQuestion();
  expect(requestQuit()).toBe(quit);
  choose(true);
  await quit;
  expect(exits()).toHaveLength(1);
});

it("names each managed-tool download or install it would stop", async () => {
  runOperation("restore");
  downloadFfmpeg();
  const quit = requestQuit();
  const choose = await presentedRunningWorkQuestion();
  expect(runningBody()).toBe("A restore is running.\nffmpeg is being downloaded.");
  choose(false);
  await quit;
  expect(exits()).toHaveLength(0);
});

it("does not ask about work the user is already cancelling", async () => {
  runOperation();
  useMutationStore.setState({ cancelling: true });
  await requestQuit();
  expect(exits()).toHaveLength(1);
  expect(useQuitRunningWorkStore.getState().body).toBeNull();
});

it("automatic derived work and source checks alone never ask", async () => {
  useDerivedWorkStore.setState({ snapshot: { revision: 1, workerRunning: true, pausedClasses: [], classes: [],
    activeItem: { id: "video-transcripts", hash: "h", done: 1, total: 9, stopping: false } } });
  useSectionsStore.setState((state) => ({ sourceCheck: { ...state.sourceCheck, running: true } }));
  try {
    await requestQuit();
    expect(exits()).toHaveLength(1);
    expect(useQuitRunningWorkStore.getState().choose).toBeNull();
  } finally {
    useDerivedWorkStore.setState({ runtime: null, snapshot: null, activeItem: null });
    useSectionsStore.setState((state) => ({ sourceCheck: { ...state.sourceCheck, running: false } }));
  }
});

it("a running-work question that cannot be shown keeps running", async () => {
  vi.useFakeTimers();
  runOperation();
  const quit = requestQuit();
  await settle();
  expect(useQuitRunningWorkStore.getState().choose).not.toBeNull();
  await vi.advanceTimersByTimeAsync(500);
  await quit;
  expect(exits()).toHaveLength(0);
  expect(useQuitRunningWorkStore.getState().choose).toBeNull();
});

it("the OS ending the session never asks about running work and settles an open question", async () => {
  runOperation();
  await endSession(4);
  expect(useQuitRunningWorkStore.getState().choose).toBeNull();
  expect(invokeCalls.find((call) => call.command === "session_end_saved")?.args).toEqual({ id: 4 });
  cancelSessionEnd();

  const quit = requestQuit();
  await presentedRunningWorkQuestion();
  await endSession(5);
  await quit;
  expect(useQuitRunningWorkStore.getState().choose).toBeNull();
  expect(exits()).toHaveLength(0);
});

it("renderer failure cancels the running-work question in Main", async () => {
  runOperation();
  const quit = requestQuit();
  await presentedRunningWorkQuestion();
  cancelQuitDecision();
  await quit;
  expect(exits()).toHaveLength(0);
});

// No question attaches to a minimized or hidden Main or brings it back: it
// stands alone as a native dialog, with the safe choice first.
function standaloneQuestions() {
  return invokeCalls.filter((call) => call.command === "quit_question").map((call) => call.args);
}

it("with Main minimized the running-work question stands alone, and Stop and quit exits", async () => {
  setCurrentWindowShown({ minimized: true });
  runOperation("delete");
  mockCommands({ quit_question: () => 1 });
  await requestQuit();
  expect(standaloneQuestions()).toEqual([{
    title: "Stop the running work and quit?",
    body: "A deletion is running.",
    labels: ["Keep running", "Stop and quit"],
  }]);
  expect(exits()).toHaveLength(1);
});

it("with Main hidden the Settings discard question stands alone and its safe answer keeps editing", async () => {
  setCurrentWindowShown({ visible: false });
  editSettings();
  let answer!: (index: number) => void;
  mockCommands({ quit_question: () => new Promise((resolve) => { answer = resolve; }) });
  const quit = requestQuit();
  await settle();
  const question = useQuitDiscardStore.getState();
  expect(question.standalone).toBe(true);
  expect(question.presented).toBeNull();
  cancelQuitDecision();
  expect(useQuitDiscardStore.getState().choose).not.toBeNull();
  answer(0);
  await quit;
  expect(standaloneQuestions()[0]?.labels).toEqual(["Keep editing", "Discard"]);
  expect(exits()).toHaveLength(0);
  expect(useSettingsStore.getState().draft?.defaultTimezone).toBe("Asia/Tokyo");
});

it("a standalone save question offers Cancel first, then Retry and Quit anyway", async () => {
  setCurrentWindowShown({ minimized: true });
  setPlaybackVolume(0.4);
  mockCommands({ save_config: () => Promise.reject(new Error("disk full")), quit_question: () => 2 });
  await requestQuit();
  expect(standaloneQuestions()[0]?.labels).toEqual(["Cancel", "Retry", "Quit anyway"]);
  expect(exits()).toHaveLength(1);
  mockCommands({ save_config: ({ changes }) => changes });
  await flushConfigForShutdown();
});

it("a standalone dialog that fails keeps running", async () => {
  setCurrentWindowShown({ minimized: true });
  runOperation();
  mockCommands({ quit_question: () => Promise.reject(new Error("no dialog")) });
  await requestQuit();
  expect(exits()).toHaveLength(0);
  expect(useQuitRunningWorkStore.getState().choose).toBeNull();
});

it("a standalone answer after the OS took over changes nothing", async () => {
  setCurrentWindowShown({ minimized: true });
  runOperation();
  let answer!: (index: number) => void;
  mockCommands({ quit_question: () => new Promise((resolve) => { answer = resolve; }) });
  const quit = requestQuit();
  await settle();
  await endSession(6);
  await quit;
  answer(1);
  await settle();
  expect(exits()).toHaveLength(0);
});

it("closes the fullscreen view the way Escape does before asking in Main, and only to ask", async () => {
  const viewing = { token: "view-1" } as unknown as NonNullable<ReturnType<typeof useFullscreenViewStore.getState>["session"]>;
  vi.mocked(closeFullscreenView).mockReset().mockImplementation(async () => { useFullscreenViewStore.setState({ session: null }); });
  useFullscreenViewStore.setState({ session: viewing });
  await requestQuit();
  expect(closeFullscreenView).not.toHaveBeenCalled();
  expect(exits()).toHaveLength(1);

  runOperation();
  const quit = requestQuit();
  await settle();
  expect(closeFullscreenView).toHaveBeenCalledWith();
  expect(useFullscreenViewStore.getState().session).toBeNull();
  (await presentedRunningWorkQuestion())(false);
  await quit;
  expect(exits()).toHaveLength(1);

  useFullscreenViewStore.setState({ session: viewing });
  setCurrentWindowShown({ minimized: true });
  mockCommands({ quit_question: () => 0 });
  await requestQuit();
  expect(closeFullscreenView).toHaveBeenCalledTimes(1);
  useFullscreenViewStore.setState({ session: null });
});

it("a quit question in Main during Comparison takes the keyboard to Main, and none is moved when nothing is asked", async () => {
  const { useComparisonStore } = await import("../../src/state/comparison-store");
  mockCommands({ focus_window_while_active: () => null });
  const focuses = () => invokeCalls.filter((call) => call.command === "focus_window_while_active").map((call) => call.args.label);
  useComparisonStore.setState({ open: true });
  try {
    await requestQuit();
    expect(focuses()).toEqual([]);
    runOperation();
    const quit = requestQuit();
    (await presentedRunningWorkQuestion())(false);
    await quit;
    expect(focuses()).toEqual(["main"]);
  } finally {
    useComparisonStore.setState({ open: false });
  }
});
