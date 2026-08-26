// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import PreviewSurface from "../../src/components/PreviewSurface";
import { useAppStore } from "../../src/state/app-store";
import { useBinariesStore } from "../../src/state/binaries-store";
import { useTranscriptStore } from "../../src/state/transcript-store";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

const DETAIL = {
  fileName: "family.mov",
  kind: "video",
  byteSize: 1_000,
  width: 1920,
  height: 1080,
  durationMs: 30_000,
  resolvedUtcMs: 0,
  resolvedSource: "metadata",
  dateOnly: false,
  copyPaths: ["/videos/family.mov"],
  companionPaths: [],
  stripFrames: 4,
};

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    transcript_get: () => ({ status: "ready", text: "[0:01] hello", message: null }),
    open_item_externally: () => null,
    background_work_snapshot: () => ({ masterPaused: false, classes: [] }),
    log_event: () => null,
  });
  useTranscriptStore.setState({ rows: {} });
  useBinariesStore.setState({ entries: [] });
  useAppStore.setState({
    appData: {
      config: {
        videoAutoplayOnShow: false,
        videoAutoplayAfterSnapshot: true,
      },
      state: null,
      dataRoot: "/app",
      debugEnabled: false,
      quarantines: [],
    },
  });
  vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue();
});

afterEach(() => {
  vi.restoreAllMocks();
  useAppStore.setState({ appData: null });
  cleanup();
});

describe("shared video presentation", () => {
  it("starts an explicit view immediately while selection-follow autoplay stays delayed", () => {
    useAppStore.setState((state) => ({
      appData: state.appData === null
        ? null
        : {
            ...state.appData,
            config: { ...state.appData.config, videoAutoplayOnShow: true },
          },
    }));

    render(
      <PreviewSurface hash="video-hash" detail={DETAIL} autoplayImmediately />,
    );

    expect(HTMLMediaElement.prototype.play).toHaveBeenCalledOnce();
  });

  it("overlays timestamped snapshots, seeks and plays, and keeps transcript below", async () => {
    const view = render(
      <PreviewSurface hash="video-hash" detail={DETAIL} keyboardActive />,
    );
    const video = view.container.querySelector("video");
    expect(video).not.toBeNull();
    expect(await screen.findByText("[0:01] hello")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Managed tools" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Play from 0:12" }));

    expect(video?.currentTime).toBe(12);
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: /Open in player/i })).toBeTruthy();

    fireEvent.keyDown(window, { key: " " });
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalledTimes(2);
  });
});
