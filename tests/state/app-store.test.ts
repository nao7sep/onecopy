import { beforeEach, describe, expect, it } from "vitest";
import { useAppStore } from "../../src/state/app-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useAppStore.setState({ appData: null, loadError: null, quarantines: [] });
});

describe("application initialization", () => {
  it("shares one startup read so a quarantine reaches the reporting surface", async () => {
    const quarantine = {
      file: "state.json",
      quarantinedTo: "/data/state-20260827-083016-254-utc.invalid",
    };
    mockCommands({
      load_app_data: () => ({
        config: { sourceDirs: [], defaultTimezone: "UTC" },
        state: null,
        dataRoot: "/data",
        debugEnabled: false,
        faceScoringSupported: true,
        transcriptionSupported: true,
        quarantines: [quarantine],
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
});
