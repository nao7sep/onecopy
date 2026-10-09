import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flushConfigForShutdown, resetConfigWritesForTests, useAppStore, retainStatePatch } from "../../src/state/app-store";
import { setSoundEnabled, setPlaybackVolume } from "../../src/workflows/playback";
import { cancelQuitDecision, cancelSessionEnd, endSession, installQuitWorkflow, requestQuit, useQuitDiscardStore, useQuitSaveStore } from "../../src/workflows/quit";
import { useSettingsStore } from "../../src/state/settings-store";
import { useAppShellStore } from "../../src/state/app-shell-store";
import { effectiveConfig } from "../helpers/config";
import { fireEvent, invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  resetConfigWritesForTests();
  cancelSessionEnd();
  useSettingsStore.setState({ draft: null, opened: null, saving: false });
  useAppShellStore.getState().closeUtility();
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
  await installQuitWorkflow();
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

// Unsaved Settings edits and an ordinary quit (developer decision): ask, and
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
