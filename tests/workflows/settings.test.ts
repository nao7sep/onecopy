// @vitest-environment happy-dom

import { beforeEach, describe, expect, it } from "vitest";
import { saveSettings } from "../../src/workflows/settings";
import { useAppStore } from "../../src/state/app-store";
import { useItemsStore } from "../../src/state/items-store";
import { useSettingsStore } from "../../src/state/settings-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

async function settleUntil(predicate: () => boolean): Promise<void> {
  for (let index = 0; index < 50 && !predicate(); index += 1) {
    await Promise.resolve();
  }
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    patch_config: () => ({}),
    patch_state: ({ patch }) => patch,
    log_event: () => null,
    re_resolve_all: () => 0,
    start_source_check: () => true,
    get_section_counts: () => ({ images: [], videos: [], others: [] }),
    check_source_dirs: () => ({ missing: [], substituted: [] }),
  });
  useSettingsStore.getState().openWith({});
  useAppStore.setState({
    appData: {
      config: {},
      state: {},
      dataRoot: "/app",
      debugEnabled: false,
      quarantines: [],
    },
  });
  useItemsStore.setState({ selected: null, message: null });
});

describe("Settings save boundary", () => {
  it("keeps the draft open when config publication itself fails", async () => {
    mockCommands({ patch_config: () => Promise.reject(new Error("disk full")) });

    await saveSettings();

    expect(useSettingsStore.getState().open).toBe(true);
    expect(useSettingsStore.getState().message).toBe(
      "Settings could not be saved. Your changes are still here; try again.",
    );
    expect(invokeCalls.some((call) => call.command === "re_resolve_all")).toBe(false);
    expect(invokeCalls.find((call) => call.command === "patch_config")?.args).toMatchObject({
      reportFailure: false,
    });
  });

  it("closes the committed draft before a large index projection finishes", async () => {
    let resolutionStarted = false;
    let finishResolution = (_value: number): void => {};
    mockCommands({
      re_resolve_all: () =>
        new Promise<number>((resolve) => {
          resolutionStarted = true;
          finishResolution = resolve;
        }),
    });

    const saving = saveSettings();
    await settleUntil(() => resolutionStarted);

    expect(useSettingsStore.getState()).toMatchObject({
      open: false,
      draft: null,
      saving: false,
    });
    finishResolution(12);
    await saving;
  });

  it("reports projection failure globally without pretending the config is unsaved", async () => {
    mockCommands({ re_resolve_all: () => Promise.reject(new Error("index unavailable")) });

    await saveSettings();

    expect(useSettingsStore.getState().open).toBe(false);
    expect(useItemsStore.getState().message).toContain(
      "Settings were saved, but the library could not be updated",
    );
  });

  it("does not check sources after saving unrelated settings", async () => {
    useSettingsStore.getState().update({ theme: "dark" });

    await saveSettings();

    expect(invokeCalls.some((call) => call.command === "start_source_check")).toBe(false);
  });

  it("checks sources when the saved source-folder list changed", async () => {
    useSettingsStore.getState().update({ sourceDirs: ["/photos"] });

    await saveSettings();

    expect(invokeCalls.some((call) => call.command === "start_source_check")).toBe(true);
  });

  it("publishes Sound and volume as view state rather than configuration", async () => {
    useSettingsStore.getState().update({ soundEnabled: false, playbackVolume: 0.35 });

    await saveSettings();

    const configSave = invokeCalls.find((call) => call.command === "patch_config");
    expect(configSave?.args.patch).not.toHaveProperty("soundEnabled");
    expect(configSave?.args.patch).not.toHaveProperty("playbackVolume");
    expect(useAppStore.getState().appData?.state).toMatchObject({
      soundEnabled: false,
      playbackVolume: 0.35,
    });
  });

  it("publishes an explicit runtime acceleration selection as configuration", async () => {
    useSettingsStore.getState().openWith(
      { aiAcceleration: { transcription: "metal", "face-scoring": "none" } },
      null,
      [
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
      ],
    );
    useSettingsStore.getState().update({
      aiAcceleration: { transcription: "none", "face-scoring": "none" },
    });

    await saveSettings();

    expect(invokeCalls.find((call) => call.command === "patch_config")?.args.patch).toMatchObject({
      aiAcceleration: { transcription: "none", "face-scoring": "none" },
    });
  });
});
