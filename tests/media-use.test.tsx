// @vitest-environment happy-dom

import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  installMediaUseBoundary,
} from "../src/media-use";
import { usePlaybackMedia } from "../src/hooks/usePlaybackMedia";
import { usePlaybackClientStore } from "../src/state/playback-client-store";
import {
  fireEvent,
  invokeCalls,
  listenerCount,
  mockCommand,
  resetTauriMocks,
} from "./mocks/tauri";

function Player({ src = "mediafile://localhost/item" }: { src?: string }) {
  const playback = usePlaybackMedia<HTMLVideoElement>("preview-window", src, "video");
  return <video ref={playback.ref} src={src} />;
}

function playingSession() {
  return { key: "mediafile://localhost/item", medium: "video" as const, owner: "preview-window" as const,
    position: 0, playing: true, soundEnabled: true, volume: 1 };
}

beforeEach(async () => {
  resetTauriMocks({ keepListeners: true });
  usePlaybackClientStore.setState({ session: null });
  mockCommand("log_event", () => null);
  mockCommand("media_use_current", () => null);
  mockCommand("media_use_released", () => true);
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => undefined);
  vi.spyOn(HTMLMediaElement.prototype, "load").mockImplementation(() => undefined);
  await installMediaUseBoundary();
  await waitFor(() => expect(listenerCount("media-use://release")).toBe(1));
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("media-use boundary", () => {
  it.each(["pause", "seek", "handoff", "pause before metadata"])(
    "restores the current %s intent instead of the release snapshot",
    async (intent) => {
      vi.useFakeTimers();
      usePlaybackClientStore.setState({ session: { ...playingSession(), position: 3 } });
      const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue();
      const view = render(<Player />);
      const video = view.container.querySelector("video")!;
      act(() => video.dispatchEvent(new Event("loadedmetadata")));
      expect(video.currentTime).toBe(3);
      expect(play).toHaveBeenCalledTimes(1);
      await act(async () => fireEvent("media-use://release", { token: 20, keys: ["other-item"] }));
      await act(async () => vi.advanceTimersByTimeAsync(50));
      play.mockClear();
      // load() in this double leaves the old readyState visible; restoration
      // must still wait for metadata belonging to the reopened source.
      Object.defineProperty(video, "readyState", { configurable: true, get: () => HTMLMediaElement.HAVE_METADATA });
      if (intent === "pause before metadata") {
        act(() => fireEvent("media-use://resume", { token: 20 }));
      }
      act(() => usePlaybackClientStore.setState({ session: {
        ...playingSession(), position: 17, playing: intent === "seek",
        owner: intent === "handoff" ? "fullscreen-view" : "preview-window",
      } }));
      if (intent !== "pause before metadata") {
        // An old metadata callback cannot apply state while the file lease is held.
        act(() => video.dispatchEvent(new Event("loadedmetadata")));
        expect(video.getAttribute("src")).toBeNull();
        expect(video.currentTime).toBe(3);
        expect(play).not.toHaveBeenCalled();
        act(() => fireEvent("media-use://resume", { token: 20 }));
      }
      expect(play).not.toHaveBeenCalled();
      act(() => video.dispatchEvent(new Event("loadedmetadata")));
      expect(video.currentTime).toBe(intent === "handoff" ? 3 : 17);
      expect(play).toHaveBeenCalledTimes(intent === "seek" ? 1 : 0);
      expect(video.getAttribute("src")).toBe("mediafile://localhost/item");
    },
  );

  it("never restores the old source into a reused media element", async () => {
    vi.useFakeTimers();
    usePlaybackClientStore.setState({ session: playingSession() });
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue();
    const view = render(<Player />);
    const video = view.container.querySelector("video")!;
    await act(async () => fireEvent("media-use://release", { token: 21, keys: ["item"] }));
    await act(async () => vi.advanceTimersByTimeAsync(50));
    const next = "mediafile://localhost/replacement";
    view.rerender(<Player src={next} />);
    expect(view.container.querySelector("video")).toBe(video);
    expect(video.getAttribute("src")).toBeNull();
    act(() => usePlaybackClientStore.setState({ session: {
      ...playingSession(), key: next, position: 8, playing: false,
    } }));
    act(() => fireEvent("media-use://resume", { token: 21 }));
    act(() => video.dispatchEvent(new Event("loadedmetadata")));
    expect(video.getAttribute("src")).toBe(next);
    expect(video.currentTime).toBe(8);
    expect(play).not.toHaveBeenCalled();
  });

  it("does not report an obsolete restoration rejection after a new pause", async () => {
    vi.useFakeTimers();
    mockCommand("record_recent_notification", () => ({}));
    usePlaybackClientStore.setState({ session: playingSession() });
    let reject!: (error: Error) => void;
    vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(() => new Promise((_resolve, fail) => { reject = fail; }));
    const view = render(<Player />);
    const video = view.container.querySelector("video")!;
    await act(async () => fireEvent("media-use://release", { token: 22, keys: ["item"] }));
    await act(async () => vi.advanceTimersByTimeAsync(50));
    act(() => fireEvent("media-use://resume", { token: 22 }));
    act(() => video.dispatchEvent(new Event("loadedmetadata")));
    act(() => usePlaybackClientStore.setState({ session: { ...playingSession(), playing: false } }));
    await act(async () => reject(new Error("obsolete start interrupted")));
    expect(invokeCalls.some((call) => call.command === "record_recent_notification")).toBe(false);
  });

  it("clears every registered player before acknowledging and restores survivors", async () => {
    vi.useFakeTimers();
    const frames: FrameRequestCallback[] = [];
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
      frames.push(callback);
      return frames.length;
    });
    const players = (late = false) => <>
      <Player src="mediafile://localhost/one" />
      <Player src="mediafile://localhost/two" />
      {late && <Player src="mediafile://localhost/three" />}
    </>;
    const view = render(players());
    let acknowledge!: (active: boolean) => void;
    mockCommand("media_use_released", () => {
      // Observe the actual acknowledgement invocation, not a later DOM.
      const videos = [...view.container.querySelectorAll("video")];
      expect(videos).toHaveLength(3);
      for (const video of videos) {
        expect(video.getAttribute("src")).toBeNull();
        expect(vi.mocked(HTMLMediaElement.prototype.pause).mock.contexts).toContain(video);
        expect(vi.mocked(HTMLMediaElement.prototype.load).mock.contexts).toContain(video);
      }
      return new Promise<boolean>((resolve) => { acknowledge = resolve; });
    });
    const acknowledgements = () => invokeCalls.filter((call) => call.command === "media_use_released");
    await act(async () => fireEvent("media-use://release", { token: 7, keys: ["one", "two"] }));
    expect(acknowledgements()).toEqual([]);
    // A reader mounted while release is held is cleared too, before paint.
    view.rerender(players(true));
    await act(async () => frames.shift()!(0));
    expect(acknowledgements()).toEqual([]);
    await act(async () => frames.shift()!(0));
    expect(acknowledgements()).toEqual([{ command: "media_use_released", args: { token: 7 } }]);
    await act(async () => acknowledge(true));
    expect([...view.container.querySelectorAll("video")].every((video) => !video.hasAttribute("src"))).toBe(true);

    act(() => fireEvent("media-use://resume", { token: 7 }));
    expect([...view.container.querySelectorAll("video")].map((video) => video.getAttribute("src"))).toEqual([
      "mediafile://localhost/one", "mediafile://localhost/two", "mediafile://localhost/three",
    ]);
  });

  it("restores an externally delegated player paused", async () => {
    usePlaybackClientStore.setState({ session: playingSession() });
    const view = render(<Player />);
    const video = view.container.querySelector("video")!;
    Object.defineProperty(video, "paused", { configurable: true, get: () => false });
    const play = vi.spyOn(video, "play").mockResolvedValue();

    await act(async () => {
      fireEvent("media-use://release", {
        token: 8,
        keys: ["item"],
        restorePlayback: false,
      });
      await new Promise((resolve) => window.setTimeout(resolve, 60));
    });
    act(() => fireEvent("media-use://resume", { token: 8, restorePlayback: false }));
    video.dispatchEvent(new Event("loadedmetadata"));

    expect(play).not.toHaveBeenCalled();
  });

  // A failed restore (seek or resumed playback) is surfaced, not swallowed.
  it("reports a restoration failure when a surviving player fails to resume playing", async () => {
    mockCommand("record_recent_notification", () => ({}));
    usePlaybackClientStore.setState({ session: playingSession() });
    const view = render(<Player />);
    const video = view.container.querySelector("video")!;
    Object.defineProperty(video, "paused", { configurable: true, get: () => false });
    vi.spyOn(video, "play").mockRejectedValue(new Error("codec unavailable"));

    await act(async () => {
      fireEvent("media-use://release", { token: 11, keys: ["item"] });
      await new Promise((resolve) => window.setTimeout(resolve, 60));
    });
    act(() => fireEvent("media-use://resume", { token: 11 }));
    await act(async () => {
      video.dispatchEvent(new Event("loadedmetadata"));
      await new Promise((resolve) => window.setTimeout(resolve, 0));
    });

    expect(
      invokeCalls.some((call) => call.command === "record_recent_notification"),
    ).toBe(true);
  });

  it("reports a restoration failure when the saved position cannot be re-applied", async () => {
    mockCommand("record_recent_notification", () => ({}));
    const view = render(<Player />);
    const video = view.container.querySelector("video")!;
    act(() => usePlaybackClientStore.setState({ session: { ...playingSession(), position: 10, playing: false } }));
    Object.defineProperty(video, "currentTime", {
      configurable: true,
      get: () => 5,
      set: () => {
        throw new Error("no seekable timeline");
      },
    });

    await act(async () => {
      fireEvent("media-use://release", { token: 12, keys: ["item"] });
      await new Promise((resolve) => window.setTimeout(resolve, 60));
    });
    act(() => fireEvent("media-use://resume", { token: 12 }));
    video.dispatchEvent(new Event("loadedmetadata"));

    expect(
      invokeCalls.some((call) => call.command === "record_recent_notification"),
    ).toBe(true);
  });

  it("restores local media when the backend cannot accept the release", async () => {
    mockCommand("media_use_released", () =>
      Promise.reject(new Error("release owner unavailable")),
    );
    const view = render(<Player />);
    const video = view.container.querySelector("video")!;

    await act(async () => {
      fireEvent("media-use://release", { token: 9, keys: ["item"] });
      await new Promise((resolve) => window.setTimeout(resolve, 60));
    });
    video.dispatchEvent(new Event("loadedmetadata"));

    expect(video.getAttribute("src")).toBe("mediafile://localhost/item");
  });
});
