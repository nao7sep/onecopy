import { beforeEach, describe, expect, it } from "vitest";
import { useSettingsStore } from "../../src/state/settings-store";
import { resetTauriMocks } from "../mocks/tauri";

const config = {
  sourceDirs: ["/photos"],
  defaultTimezone: "Asia/Tokyo",
};

beforeEach(() => {
  resetTauriMocks();
  useSettingsStore.getState().beginEditing(config);
});

describe("playback preferences", () => {
  it("defaults separate autoplay and missing playback state on", () => {
    expect(useSettingsStore.getState().draft).toMatchObject({
      videoAutoplay: true,
      audioAutoplay: true,
      soundEnabled: true,
      playbackVolume: 1,
      enlargeSmallImagesInPreview: true,
      enlargeSmallImagesInQuickView: true,
      textPreviewMaxBytes: 2 * 1024 * 1024,
      textFallbackEncoding: "utf-8",
    });
  });

  it("preserves explicit off choices", () => {
    useSettingsStore.getState().beginEditing(
      {
        ...config,
        videoAutoplay: false,
        audioAutoplay: false,
      },
      {
        soundEnabled: false,
        playbackVolume: 0.4,
      },
    );
    expect(useSettingsStore.getState().draft).toMatchObject({
      videoAutoplay: false,
      audioAutoplay: false,
      soundEnabled: false,
      playbackVolume: 0.4,
    });
  });

  it("does not read playback state from the configuration document", () => {
    useSettingsStore.getState().beginEditing({
      ...config,
      soundEnabled: false,
      playbackVolume: 0.2,
    });

    expect(useSettingsStore.getState().draft).toMatchObject({
      soundEnabled: true,
      playbackVolume: 1,
    });
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
  it("presents the historical seeded CSS stack as the blank system default", () => {
    useSettingsStore.getState().beginEditing({
      ...config,
      uiFontFamily:
        'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif',
    });
    expect(useSettingsStore.getState().draft?.uiFontFamily).toBe("");
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
