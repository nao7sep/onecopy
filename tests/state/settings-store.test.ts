import { beforeEach, describe, expect, it } from "vitest";
import { useSettingsStore } from "../../src/state/settings-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

const config = {
  sourceDirs: ["/photos"],
  defaultTimezone: "Asia/Tokyo",
};

beforeEach(() => {
  resetTauriMocks();
  useSettingsStore.getState().openWith(config);
});

describe("settings timezone validation", () => {
  it("settles blank input as invalid without invoking the backend", async () => {
    await useSettingsStore.getState().validateTimezone("   ");

    expect(useSettingsStore.getState()).toMatchObject({
      timezoneValid: false,
      timezonePending: false,
    });
    expect(invokeCalls).toEqual([]);
  });

  it("does not let a reply from an earlier modal session overwrite a reopened draft", async () => {
    let settle: ((valid: boolean) => void) | undefined;
    mockCommands({
      validate_timezone: () =>
        new Promise<boolean>((resolve) => {
          settle = resolve;
        }),
    });

    const validation = useSettingsStore.getState().validateTimezone("Tokyo");
    useSettingsStore.getState().openWith(config);
    settle?.(false);
    await validation;

    expect(useSettingsStore.getState()).toMatchObject({
      timezoneValid: true,
      timezonePending: false,
    });
    expect(useSettingsStore.getState().draft?.defaultTimezone).toBe(
      "Asia/Tokyo",
    );
  });
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
    useSettingsStore.getState().openWith(
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
    useSettingsStore.getState().openWith({
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

describe("UI font preference", () => {
  it("presents the historical seeded CSS stack as the blank system default", () => {
    useSettingsStore.getState().openWith({
      ...config,
      uiFontFamily:
        'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif',
    });
    expect(useSettingsStore.getState().draft?.uiFontFamily).toBe("");
  });

  it("preserves a custom family list", () => {
    useSettingsStore.getState().openWith({
      ...config,
      uiFontFamily: "  Iosevka, monospace  ",
    });
    expect(useSettingsStore.getState().draft?.uiFontFamily).toBe(
      "  Iosevka, monospace  ",
    );
  });
});
