import { beforeEach, describe, expect, it } from "vitest";
import { useSettingsStore } from "../../src/state/settings-store";
import { resetTauriMocks } from "../mocks/tauri";
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

describe("defaults", () => {
  it("confirms a single-item Delete by default, as the core's defaults say (R7-07)", () => {
    expect(useSettingsStore.getState().draft?.confirmTrashDelete).toBe(true);
  });

  it("supplies no default of its own for a missing member", () => {
    const { maximumImagesInComparison: _omitted, ...partial } = config;
    expect(() => useSettingsStore.getState().beginEditing(partial)).toThrow(
      "maximumImagesInComparison must be a number.",
    );
  });
});

describe("GitHub release preference", () => {
  it("defaults on and preserves an explicit false value", () => {
    expect(useSettingsStore.getState().draft?.checkGithubReleasesAtLaunch).toBe(true);
    useSettingsStore.getState().beginEditing({
      ...config,
      checkGithubReleasesAtLaunch: false,
    });
    expect(useSettingsStore.getState().draft?.checkGithubReleasesAtLaunch).toBe(false);
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
