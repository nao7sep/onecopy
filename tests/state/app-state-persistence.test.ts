// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { retainStatePatch, useAppStore } from "../../src/state/app-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  vi.useFakeTimers();
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    patch_state: ({ patch }) => patch,
    publish_notification: ({ request }) => ({
      id: 1,
      ...(request as Record<string, unknown>),
    }),
    log_event: () => null,
  });
  useAppStore.setState({
    appData: {
      config: {},
      state: {},
      dataRoot: "/data",
      debugEnabled: false,
      quarantines: [],
    },
  });
});

afterEach(() => vi.useRealTimers());

describe("app state persistence settlement", () => {
  it("coalesces writes but settles every caller only after the disk boundary", async () => {
    const first = useAppStore.getState().patchState({ sidebarWidth: 300 });
    const second = useAppStore.getState().patchState({ rightPaneWidth: 320 });
    let settled = false;
    void Promise.all([first, second]).then(() => { settled = true; });
    await Promise.resolve();
    expect(settled).toBe(false);

    await vi.advanceTimersByTimeAsync(400);
    await expect(Promise.all([first, second])).resolves.toEqual([undefined, undefined]);
    const writes = invokeCalls.filter((call) => call.command === "patch_state");
    expect(writes).toHaveLength(1);
    expect(writes[0]?.args.patch).toEqual({ sidebarWidth: 300, rightPaneWidth: 320 });
  });

  it("rejects an explicitly owned state mutation with its original diagnostic", async () => {
    const hostile = new TypeError("EACCES /private/tmp/HOSTILE-SENTINEL IPC wrapper");
    mockCommands({ patch_state: () => Promise.reject(hostile) });
    const saving = useAppStore.getState().patchState({ soundEnabled: false });
    const rejected = expect(saving).rejects.toBe(hostile);

    await vi.advanceTimersByTimeAsync(400);

    await rejected;
  });

  it("retains passive write failure as authored app-owned copy", async () => {
    mockCommands({
      patch_state: () => Promise.reject(
        new TypeError("EACCES /private/tmp/HOSTILE-SENTINEL IPC wrapper"),
      ),
    });
    retainStatePatch({ zoomLevel: 1.2 });

    await vi.advanceTimersByTimeAsync(400);
    await vi.waitFor(() => {
      expect(invokeCalls.some((call) => call.command === "publish_notification")).toBe(true);
    });
    const notification = invokeCalls.find((call) => call.command === "publish_notification");
    expect(notification?.args.request).toMatchObject({
      kind: "interface-state-save-failed",
      level: "error",
      presentation: "persistent",
      message: "OneCopy couldn’t save the current interface state. Your changes remain available in this session.",
    });
    expect(JSON.stringify(notification?.args.request)).not.toMatch(
      /EACCES|HOSTILE-SENTINEL|TypeError|IPC|private\/tmp/,
    );
  });
});
