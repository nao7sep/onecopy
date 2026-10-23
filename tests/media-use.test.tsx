// @vitest-environment happy-dom

import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  installMediaUseBoundary,
  useOwnedMedia,
} from "../src/media-use";
import {
  fireEvent,
  invokeCalls,
  listenerCount,
  mockCommand,
  resetTauriMocks,
} from "./mocks/tauri";

function Player({ src = "mediafile://localhost/item" }: { src?: string }) {
  const [, setRef] = useOwnedMedia<HTMLVideoElement>();
  return <video ref={setRef} src={src} />;
}

beforeEach(async () => {
  resetTauriMocks({ keepListeners: true });
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
