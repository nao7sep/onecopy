// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PlaybackSession } from "../../src/models/playback";
import { emitCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

const playbackClient = vi.hoisted(() => ({
  register: vi.fn<() => Promise<void>>(),
  unregister: vi.fn(),
  session: null as PlaybackSession | null,
}));

vi.mock("../../src/state/playback-client-store", () => ({
  registerPlaybackClient: playbackClient.register,
  unregisterPlaybackClient: playbackClient.unregister,
  usePlaybackClientStore: (select: (state: { session: PlaybackSession | null }) => unknown) =>
    select({ session: playbackClient.session }),
}));

import { usePlaybackMedia } from "../../src/hooks/usePlaybackMedia";

function Harness() {
  const playback = usePlaybackMedia<HTMLAudioElement>("quick", "audio-key", "audio");
  return playback.setupFailed ? (
    <div role="alert">
      <span>Playback controls could not be connected. Try again.</span>
      <button type="button" onClick={() => void playback.retrySetup()}>Retry</button>
    </div>
  ) : null;
}

beforeEach(() => mockCommands({ log_event: () => null }));

afterEach(() => {
  cleanup();
  playbackClient.register.mockReset();
  playbackClient.unregister.mockReset();
  playbackClient.session = null;
  vi.restoreAllMocks();
  resetTauriMocks();
});

function MediaHarness({ enabled = true }: { enabled?: boolean }) {
  const playback = usePlaybackMedia<HTMLAudioElement>("quick", "audio-key", "audio", enabled);
  return <audio ref={playback.ref} />;
}

function playingSession(): PlaybackSession {
  return {
    key: "audio-key", medium: "audio", owner: "quick", position: 0,
    playing: true, soundEnabled: true, volume: 1,
  };
}

it.each(["newer attempt", "owner handoff", "disabled", "unmounted"])(
  "ignores an obsolete play rejection after %s",
  async (transition) => {
    playbackClient.session = playingSession();
    let rejectPlay!: (error: Error) => void;
    const play = vi.spyOn(HTMLMediaElement.prototype, "play")
      .mockImplementationOnce(() => new Promise<void>((_resolve, reject) => { rejectPlay = reject; }))
      .mockResolvedValue();
    const view = render(<MediaHarness />);
    fireEvent.loadedMetadata(view.container.querySelector("audio")!);
    expect(play).toHaveBeenCalledTimes(1);
    if (transition === "unmounted") view.unmount();
    else {
      playbackClient.session = {
        ...playingSession(),
        owner: transition === "owner handoff" ? "viewer" : "quick",
        position: 12,
      };
      view.rerender(<MediaHarness enabled={transition !== "disabled"} />);
      fireEvent.loadedMetadata(view.container.querySelector("audio")!);
    }
    await act(async () => rejectPlay(new DOMException("Previous playback interrupted", "AbortError")));
    expect(emitCalls.filter((call) => call.event === "playback://observe")).toEqual([]);
  },
);

it("reports a failure from the current play attempt as paused", async () => {
  playbackClient.session = playingSession();
  vi.spyOn(HTMLMediaElement.prototype, "play").mockRejectedValue(new Error("Playback denied"));
  const view = render(<MediaHarness />);
  fireEvent.loadedMetadata(view.container.querySelector("audio")!);
  await act(async () => {});
  expect(emitCalls.filter((call) => call.event === "playback://observe")).toEqual([
    expect.objectContaining({ payload: expect.objectContaining({ key: "audio-key", playing: false }) }),
  ]);
});

describe("usePlaybackMedia setup ownership", () => {
  it("retains authored local recovery and clears it only after a matching retry", async () => {
    playbackClient.register
      .mockRejectedValueOnce(new Error("EACCES /private/tmp/PLAYBACK IPC sentinel"))
      .mockResolvedValueOnce(undefined);

    render(<Harness />);

    const result = await screen.findByRole("alert");
    expect(result.textContent).toContain("Playback controls could not be connected");
    expect(result.textContent).not.toMatch(/EACCES|private\/tmp|IPC sentinel/);

    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Retry" })));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(playbackClient.register).toHaveBeenCalledTimes(2);
  });
});
