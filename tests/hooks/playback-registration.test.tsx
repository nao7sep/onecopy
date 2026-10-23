// @vitest-environment happy-dom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { usePlaybackMedia } from "../../src/hooks/usePlaybackMedia";
import { installPlaybackClient } from "../../src/state/playback-client-store";
import { emit, emitCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

function Media() {
  const playback = usePlaybackMedia<HTMLAudioElement>("preview-window", "clip", "audio");
  return playback.setupFailed ? <button onClick={() => void playback.retrySetup()}>Retry playback</button> : null;
}

afterEach(cleanup);

it("offers recovery when an installed client cannot deliver a new registration", async () => {
  resetTauriMocks();
  mockCommands({ log_event: () => null });
  await installPlaybackClient();
  emit.mockRejectedValueOnce(new Error("registration delivery failed"));
  render(<Media />);
  const retry = await screen.findByRole("button", { name: "Retry playback" });
  await act(async () => fireEvent.click(retry));
  expect(screen.queryByRole("button", { name: "Retry playback" })).toBeNull();
  expect(emitCalls.filter((call) => call.event === "playback://register")).toEqual([
    { event: "playback://register", payload: { surface: "preview-window", key: "clip", medium: "audio" } },
  ]);
});
