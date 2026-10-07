import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useAppStore, retainStatePatch } from "../../src/state/app-store";
import { setSoundEnabled, setPlaybackVolume, flushPlaybackConfigForShutdown } from "../../src/workflows/playback";
import { cancelQuitDecision, cancelSessionEnd, endSession, installQuitWorkflow, requestQuit, useQuitSaveStore } from "../../src/workflows/quit";
import { fireEvent, invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  cancelSessionEnd();
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
  await flushPlaybackConfigForShutdown();
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
  await flushPlaybackConfigForShutdown();
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
  await flushPlaybackConfigForShutdown();
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
  await flushPlaybackConfigForShutdown();
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
  await flushPlaybackConfigForShutdown();
});
