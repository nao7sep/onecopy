// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
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

  it("pauses a playing video during held inspection and resumes it on release", () => {
    vi.useFakeTimers();
    const view = render(<PreviewSurface hash="video-hash" detail={DETAIL} />);
    const video = view.container.querySelector("video")!;
    let paused = false;
    Object.defineProperty(video, "paused", { configurable: true, get: () => paused });
    const pause = vi.spyOn(video, "pause").mockImplementation(() => {
      paused = true;
    });
    const play = vi.spyOn(video, "play").mockImplementation(() => {
      paused = false;
      return Promise.resolve();
    });
    const viewport = screen.getByTitle("Press and hold the picture for original pixels");

    fireEvent.pointerDown(viewport, {
      pointerId: 8,
      button: 0,
      isPrimary: true,
      clientX: 20,
      clientY: 20,
    });
    act(() => vi.advanceTimersByTime(135));

    expect(pause).toHaveBeenCalledOnce();
    expect(video.controls).toBe(false);

    fireEvent.pointerUp(window, { pointerId: 8, clientX: 40, clientY: 30 });

    expect(play).toHaveBeenCalledOnce();
    expect(video.controls).toBe(true);
    vi.useRealTimers();
  });

  it("cancels delayed autoplay when inspection starts first", () => {
    vi.useFakeTimers();
    useAppStore.setState((state) => ({
      appData: state.appData === null
        ? null
        : {
            ...state.appData,
            config: { ...state.appData.config, videoAutoplayOnShow: true },
          },
    }));
    render(<PreviewSurface hash="video-hash" detail={DETAIL} />);
    const viewport = screen.getByTitle("Press and hold the picture for original pixels");

    fireEvent.pointerDown(viewport, { pointerId: 9, button: 0, isPrimary: true });
    act(() => vi.advanceTimersByTime(135));
    act(() => vi.advanceTimersByTime(500));

    expect(HTMLMediaElement.prototype.play).not.toHaveBeenCalled();
    fireEvent.pointerUp(window, { pointerId: 9 });
    expect(HTMLMediaElement.prototype.play).not.toHaveBeenCalled();
    vi.useRealTimers();
  });

  it("defers snapshot playback until a held frame is released", () => {
    vi.useFakeTimers();
    const view = render(
      <PreviewSurface hash="video-hash" detail={DETAIL} seekMs={12_000} />,
    );
    const video = view.container.querySelector("video")!;
    Object.defineProperty(video, "readyState", {
      configurable: true,
      value: HTMLMediaElement.HAVE_NOTHING,
    });
    const viewport = screen.getByTitle("Press and hold the picture for original pixels");
    fireEvent.pointerDown(viewport, { pointerId: 10, button: 0, isPrimary: true });
    act(() => vi.advanceTimersByTime(135));

    fireEvent(video, new Event("loadedmetadata"));
    expect(HTMLMediaElement.prototype.play).not.toHaveBeenCalled();

    fireEvent.pointerUp(window, { pointerId: 10 });
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalledOnce();
    vi.useRealTimers();
  });
});
