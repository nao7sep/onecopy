import { beforeEach, describe, expect, it } from "vitest";
import { useTranscriptStore } from "../../src/state/transcript-store";
import { fireEvent, mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  fireEvent("transcribe://cancelled", { hash: "background-video" });
  useTranscriptStore.setState({ rows: {} });
  mockCommands({
    transcript_get: () => ({ status: "pending", text: null, message: null }),
  });
});

describe("transcript projection", () => {
  it("tracks unseen automatic work without caching every library item", async () => {
    fireEvent("transcribe://progress", { hash: "background-video", percent: 23 });
    expect(useTranscriptStore.getState().rows).toEqual({});

    await useTranscriptStore.getState().load("background-video");
    expect(useTranscriptStore.getState().rows["background-video"]).toMatchObject({
      status: "running",
      percent: 23,
    });

    fireEvent("transcribe://done", { hash: "background-video", text: "hello" });
    expect(useTranscriptStore.getState().rows["background-video"]).toMatchObject({
      status: "ready",
      text: "hello",
    });
  });
});
