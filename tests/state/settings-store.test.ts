import { beforeEach, describe, expect, it } from "vitest";
import { useSettingsStore } from "../../src/state/settings-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";
import { effectiveConfig } from "../helpers/config";

const config = effectiveConfig({
  sourceDirs: ["/photos"],
  defaultTimezone: "Asia/Tokyo",
});

beforeEach(() => {
  resetTauriMocks();
  useSettingsStore.getState().beginEditing(config);
});

describe("playback preferences", () => {
  it("reads the core's defaults and starts missing playback state on", () => {
    expect(useSettingsStore.getState().draft).toMatchObject({
      autoplay: true,
      soundEnabled: true,
      playbackVolume: 1,
      enlargeSmallImages: true,
      textFallbackEncoding: "utf-8",
    });
  });

  it("preserves explicit off choices", () => {
    useSettingsStore.getState().beginEditing(
      {
        ...config,
        autoplay: false,
        soundEnabled: false,
        playbackVolume: 0.4,
      },
    );
    expect(useSettingsStore.getState().draft).toMatchObject({
      autoplay: false,
      soundEnabled: false,
      playbackVolume: 0.4,
    });
  });

  it("clamps a stored volume into the playable range", () => {
    useSettingsStore.getState().beginEditing({ ...config, playbackVolume: 0 });
    expect(useSettingsStore.getState().draft?.playbackVolume).toBe(0.01);
  });
});

describe("time zones", () => {
  it("drafts the core's zones as they are, and a missing display zone as System", () => {
    expect(useSettingsStore.getState().draft).toMatchObject({
      defaultTimezone: "Asia/Tokyo",
      displayTimezone: "system",
    });
    const { displayTimezone: _omitted, ...partial } = config;
    useSettingsStore.getState().beginEditing(partial);
    expect(useSettingsStore.getState().draft?.displayTimezone).toBe("system");
    useSettingsStore.getState().beginEditing({ ...config, displayTimezone: "Europe/Paris" });
    expect(useSettingsStore.getState().draft?.displayTimezone).toBe("Europe/Paris");
  });
});

describe("defaults", () => {
  it("confirms a single-item Delete by default, as the core's defaults say", () => {
    expect(useSettingsStore.getState().draft?.confirmTrashDelete).toBe(true);
  });

  it("supplies no default of its own for a missing member", () => {
    const { maximumImagesInComparison: _omitted, ...partial } = config;
    expect(() => useSettingsStore.getState().beginEditing(partial)).toThrow(
      "maximumImagesInComparison must be a number.",
    );
  });
});

describe("UI font preference", () => {
  it("preserves a saved font stack verbatim", () => {
    useSettingsStore.getState().beginEditing({
      ...config,
      uiFontFamily:
        'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif',
    });
    expect(useSettingsStore.getState().draft?.uiFontFamily).toBe('system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif');
  });

  it("preserves a custom family list", () => {
    useSettingsStore.getState().beginEditing({
      ...config,
      uiFontFamily: "  Iosevka, monospace  ",
    });
    expect(useSettingsStore.getState().draft?.uiFontFamily).toBe(
      "  Iosevka, monospace  ",
    );
  });
});


it("retains exact native picker identities in the settings draft and source-check IPC", async () => {
  const paths = ["/photos/\u0000bfe", "/photos/\u0000bff", "/photos/\ufffd"];
  mockCommands({ pick_directories: () => paths, package_source_dirs: () => [] });
  await useSettingsStore.getState().addSourceDir();
  await useSettingsStore.getState().addSourceDir();
  expect(useSettingsStore.getState().draft?.sourceDirs).toEqual(["/photos", ...paths]);
  expect(invokeCalls.find((call) => call.command === "pick_directories")?.args).toEqual({ multiple: true });
  expect(invokeCalls.find((call) => call.command === "package_source_dirs")?.args.paths).toEqual(paths);
  mockCommands({ pick_directories: () => [] });
  await useSettingsStore.getState().addSourceDir();
  expect(useSettingsStore.getState().draft?.sourceDirs).toEqual(["/photos", ...paths]);
});
