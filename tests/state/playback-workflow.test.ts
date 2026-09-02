import { beforeEach, describe, expect, it } from "vitest";
import type { PlaybackSession } from "../../src/models/playback";
import { useAppStore } from "../../src/state/app-store";
import { usePreviewStore } from "../../src/state/preview-store";
import { useQuickViewStore } from "../../src/state/quick-view-store";
import { installPlaybackWorkflow } from "../../src/workflows/playback";
import { emitCalls, fireEvent, resetTauriMocks } from "../mocks/tauri";

function latestState(): PlaybackSession | null {
  const call = [...emitCalls]
    .reverse()
    .find((entry) => entry.event === "playback://state");
  return (call?.payload as PlaybackSession | null | undefined) ?? null;
}

beforeEach(async () => {
  resetTauriMocks({ keepListeners: true });
  useAppStore.setState({
    appData: {
      config: {
        videoAutoplay: true,
        audioAutoplay: false,
      },
      state: {
        soundEnabled: true,
        playbackVolume: 0.7,
      },
      dataRoot: "/app",
      debugEnabled: false,
      faceScoringSupported: true,
      transcriptionSupported: true,
      quarantines: [],
    },
  });
  usePreviewStore.setState({ follow: true });
  useQuickViewStore.setState({ session: null });
  await installPlaybackWorkflow();
});

describe("playback workflow", () => {
  it("hands one live session to the highest surface and back without losing state", () => {
    fireEvent("playback://register", {
      surface: "preview-split",
      key: "clip",
      medium: "video",
    });
    expect(latestState()).toMatchObject({
      owner: "preview-split",
      key: "clip",
      playing: true,
      volume: 0.7,
    });

    fireEvent("playback://register", {
      surface: "quick",
      key: "clip",
      medium: "video",
    });
    fireEvent("playback://observe", {
      surface: "quick",
      key: "clip",
      position: 12.5,
      playing: false,
      volume: 0.7,
      muted: false,
    });
    fireEvent("playback://unregister", {
      surface: "quick",
      key: "clip",
      medium: "video",
    });

    expect(latestState()).toMatchObject({
      owner: "preview-split",
      key: "clip",
      position: 12.5,
      playing: false,
    });
    fireEvent("playback://unregister", {
      surface: "preview-split",
      key: "clip",
      medium: "video",
    });
  });

  it("pauses the live session before external delegation", () => {
    fireEvent("playback://register", {
      surface: "preview-split",
      key: "external-clip",
      medium: "video",
    });
    fireEvent("playback://pause", { key: "external-clip" });

    expect(latestState()).toMatchObject({
      owner: "preview-split",
      key: "external-clip",
      playing: false,
    });
    fireEvent("playback://unregister", {
      surface: "preview-split",
      key: "external-clip",
      medium: "video",
    });
  });

  it("retains an ownerless session only for a real same-item surface handoff", () => {
    usePreviewStore.setState({
      follow: true,
      current: { hash: "handoff", pathId: null, detail: null },
    });
    fireEvent("playback://register", {
      surface: "preview-split",
      key: "handoff",
      medium: "video",
    });
    fireEvent("playback://unregister", {
      surface: "preview-split",
      key: "handoff",
      medium: "video",
    });
    expect(latestState()).toMatchObject({ key: "handoff", owner: null });

    usePreviewStore.setState({ current: null });
    fireEvent("playback://register", {
      surface: "preview-split",
      key: "leaving",
      medium: "video",
    });
    fireEvent("playback://unregister", {
      surface: "preview-split",
      key: "leaving",
      medium: "video",
    });
    expect(latestState()).toBeNull();
  });
});
