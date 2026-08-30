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

function Player() {
  const [, setRef] = useOwnedMedia<HTMLVideoElement>();
  return <video ref={setRef} src="mediafile://localhost/item" />;
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
});

describe("media-use boundary", () => {
  it("clears every registered player before acknowledging and restores survivors", async () => {
    const view = render(<Player />);
    const video = view.container.querySelector("video")!;
    expect(video.getAttribute("src")).toBe("mediafile://localhost/item");

    await act(async () => {
      fireEvent("media-use://release", { token: 7, keys: ["item"] });
      await new Promise((resolve) => window.setTimeout(resolve, 60));
    });

    expect(video.getAttribute("src")).toBeNull();
    expect(invokeCalls).toContainEqual({
      command: "media_use_released",
      args: { token: 7 },
    });

    act(() => fireEvent("media-use://resume", { token: 7 }));
    expect(video.getAttribute("src")).toBe("mediafile://localhost/item");
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
});
