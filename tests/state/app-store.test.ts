// @vitest-environment happy-dom

import { beforeEach, describe, expect, it } from "vitest";
import { useAppStore } from "../../src/state/app-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useAppStore.setState({ appData: null, startupFailure: null, quarantines: [] });
});

describe("application initialization", () => {
  it("shares one startup read so a quarantine reaches the reporting surface", async () => {
    const quarantine = {
      file: "state.json",
      quarantinedTo: "/data/state-20260827-083016-254-utc.invalid",
    };
    mockCommands({
      load_app_data: () => ({
        status: "ready",
        data: {
          config: { sourceDirs: [], defaultTimezone: "UTC" },
          state: null,
          dataRoot: "/data",
          debugEnabled: false,
          quarantines: [quarantine],
        },
      }),
      log_event: () => null,
      logging_debug_enabled: () => false,
    });

    const initialize = useAppStore.getState().initialize;
    const [appearance, bootstrap] = await Promise.all([initialize(), initialize()]);

    expect(appearance).toBe(bootstrap);
    expect(invokeCalls.filter((call) => call.command === "load_app_data")).toHaveLength(1);
    expect(useAppStore.getState().quarantines).toEqual([quarantine]);

    await initialize();
    expect(invokeCalls.filter((call) => call.command === "load_app_data")).toHaveLength(1);
  });

  it("keeps a blocked bootstrap terminal and does not repeat the backend request", async () => {
    mockCommands({
      load_app_data: () => ({
        status: "blocked",
        failure: {
          title: "OneCopy could not start safely",
          message: "Your photos were not changed.",
        },
      }),
    });

    const initialize = useAppStore.getState().initialize;
    expect(await initialize()).toBeNull();
    expect(await initialize()).toBeNull();
    expect(invokeCalls.filter((call) => call.command === "load_app_data")).toHaveLength(1);
    expect(useAppStore.getState().startupFailure).toEqual({
      title: "OneCopy could not start safely",
      message: "Your photos were not changed.",
    });
  });
});
