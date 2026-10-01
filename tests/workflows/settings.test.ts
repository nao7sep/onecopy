// @vitest-environment happy-dom

import { beforeEach, describe, expect, it } from "vitest";
import { saveSettings } from "../../src/workflows/settings";
import { useAppStore } from "../../src/state/app-store";
import { useItemsStore } from "../../src/state/items-store";
import { useSettingsStore } from "../../src/state/settings-store";
import { useAppShellStore } from "../../src/state/app-shell-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";
import { inEnglish } from "../helpers/i18n";
import { effectiveConfig } from "../helpers/config";

async function settleUntil(predicate: () => boolean): Promise<void> {
  for (let index = 0; index < 50 && !predicate(); index += 1) {
    await Promise.resolve();
  }
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    patch_config: () => effectiveConfig(),
    patch_state: ({ patch }) => patch,
    log_event: () => null,
    apply_library_settings: () => ({ status: "applied", resolved: 0 }),
    start_source_check: () => true,
    get_section_counts: () => ({ images: [], videos: [], others: [] }),
    check_source_dirs: () => ({ missing: [], substituted: [] }),
  });
  useSettingsStore.getState().beginEditing(effectiveConfig());
  useAppShellStore.getState().openUtility("settings");
  useAppStore.setState({
    appData: {
      config: effectiveConfig(),
      state: {},
      dataRoot: "/app",
      debugEnabled: false,
      quarantines: [],
    },
  });
  useItemsStore.setState({ selected: null });
});

describe("Settings save boundary", () => {
  it("changes visibility without rechecking sources", async () => {
    useSettingsStore.getState().update({ hideDotNames: false, ignoredFileNames: [] });
    await saveSettings();
    expect(invokeCalls.some((call) => call.command === "apply_library_settings")).toBe(true);
    expect(invokeCalls.some((call) => call.command === "start_source_check")).toBe(false);
    expect(invokeCalls.find((call) => call.command === "patch_config")?.args).toMatchObject({ patch: { hideDotNames: false, ignoredFileNames: [] } });
  });

  it("writes one changed set and leaves every other set absent", async () => {
    useSettingsStore.getState().update({ previewLongEdgePx: 2000 });
    await saveSettings();
    expect(invokeCalls.find((call) => call.command === "patch_config")?.args.patch).toEqual({ previewLongEdgePx: 2000 });
  });

  it("saves the similarity reset as deletion", async () => {
    useSettingsStore.getState().beginEditing(effectiveConfig({ similarity: { maxGapSeconds: 12, phashMaxDistance: 19, phashMaxDistanceBurst: 27, diameterMultiplier: 4 } }), [], effectiveConfig());
    useSettingsStore.getState().resetSimilarPhotoSettings();
    await saveSettings();
    expect(invokeCalls.find((call) => call.command === "patch_config")?.args.patch).toEqual({ similarity: null });
  });

  it("says a busy apply will still take effect instead of reporting a failure", async () => {
    mockCommands({ apply_library_settings: () => ({ status: "owed" }) });
    useSettingsStore.getState().update({ defaultTimezone: "UTC" });

    await saveSettings();

    const notice = invokeCalls.find((call) => call.command === "publish_notification")?.args.request;
    expect(notice).toMatchObject({
      level: "info",
      presentation: "timed",
      messageKey: "settings.applyOwedNotice",
    });
    await settleUntil(() => invokeCalls.some((call) => call.command === "activity_record" && (call.args.draft as { kind: string }).kind !== "started"));
    expect(invokeCalls.filter((call) => call.command === "activity_record").map((call) => (call.args.draft as { kind: string }).kind)).toEqual(["started", "completed"]);
  });
  it("keeps the draft open when config publication itself fails", async () => {
    mockCommands({ patch_config: () => Promise.reject(new Error("disk full")) });

    await saveSettings();

    expect(useAppShellStore.getState().utilitySurface).toBe("settings");
    expect(inEnglish(useSettingsStore.getState().message)).toBe(
      "Settings could not be saved. Your changes are still here; try again.",
    );
    expect(invokeCalls.some((call) => call.command === "apply_library_settings")).toBe(false);
    expect(invokeCalls.find((call) => call.command === "patch_config")?.args).toMatchObject({
      reportFailure: false,
    });
  });

  it("closes the committed draft before a large index projection finishes", async () => {
    let resolutionStarted = false;
    let finishResolution = (_value: number): void => {};
    mockCommands({
      apply_library_settings: () =>
        new Promise((resolve) => {
          resolutionStarted = true;
          finishResolution = (resolved) => resolve({ status: "applied", resolved });
        }),
    });

    const saving = saveSettings();
    await settleUntil(() => resolutionStarted);

    expect(useSettingsStore.getState()).toMatchObject({
      draft: null,
      saving: false,
    });
    finishResolution(12);
    await saving;
  });

  it("reports projection failure globally without pretending the config is unsaved", async () => {
    mockCommands({ apply_library_settings: () => Promise.reject(new Error("index unavailable")) });

    await saveSettings();

    expect(useAppShellStore.getState().utilitySurface).toBeNull();
    expect(invokeCalls.find((call) => call.command === "publish_notification")?.args.request).toMatchObject({
      messageKey: "settings.reindexFailedNotice",
      presentation: "persistent",
    });
    await settleUntil(() => invokeCalls.some((call) => call.command === "activity_record" && (call.args.draft as { kind: string }).kind === "failed"));
    expect(invokeCalls.filter((call) => call.command === "activity_record").map((call) => (call.args.draft as { kind: string }).kind)).toEqual(["started", "failed"]);
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

  it("publishes Sound and volume with the rest of the config patch and writes no state", async () => {
    useSettingsStore.getState().update({ soundEnabled: false, playbackVolume: 0.35 });

    await saveSettings();

    const configSave = invokeCalls.find((call) => call.command === "patch_config");
    expect(configSave?.args.patch).toEqual({
      soundEnabled: false,
      playbackVolume: 0.35,
    });
    expect(invokeCalls.some((call) => call.command === "patch_state")).toBe(false);
  });

  it("publishes an explicit runtime acceleration selection as configuration", async () => {
    useSettingsStore.getState().beginEditing(
      effectiveConfig({ aiAcceleration: { transcription: "metal", "face-scoring": "none" } }),
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
