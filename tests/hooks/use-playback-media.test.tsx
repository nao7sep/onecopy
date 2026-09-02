// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const playbackClient = vi.hoisted(() => ({
  register: vi.fn<() => Promise<void>>(),
  unregister: vi.fn(),
}));

vi.mock("../../src/state/playback-client-store", () => ({
  registerPlaybackClient: playbackClient.register,
  unregisterPlaybackClient: playbackClient.unregister,
  usePlaybackClientStore: (select: (state: { session: null }) => unknown) => select({ session: null }),
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

afterEach(() => {
  cleanup();
  playbackClient.register.mockReset();
  playbackClient.unregister.mockReset();
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
