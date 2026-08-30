import { beforeEach, describe, expect, it } from "vitest";
import { useTranscriptStore } from "../../src/state/transcript-store";
import {
  fireEvent,
  invokeCalls,
  mockCommands,
  resetTauriMocks,
} from "../mocks/tauri";

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
    fireEvent("transcribe://progress", {
      hash: "background-video",
      percent: 23,
    });
    expect(useTranscriptStore.getState().rows).toEqual({});

    await useTranscriptStore.getState().load("background-video");
    expect(
      useTranscriptStore.getState().rows["background-video"],
    ).toMatchObject({
      status: "running",
      percent: 23,
    });

    fireEvent("transcribe://done", { hash: "background-video", text: "hello" });
    expect(
      useTranscriptStore.getState().rows["background-video"],
    ).toMatchObject({
      status: "ready",
      text: "hello",
    });
  });

  it("keeps a second manual request queued until its own progress begins", async () => {
    mockCommands({
      transcript_get: () => ({ status: "pending", text: null, message: null }),
      transcribe: () => null,
    });

    await useTranscriptStore.getState().start("first");
    await useTranscriptStore.getState().start("second");

    expect(useTranscriptStore.getState().rows.first?.status).toBe("queued");
    expect(useTranscriptStore.getState().rows.second?.status).toBe("queued");

    fireEvent("transcribe://progress", { hash: "first", percent: 0 });
    expect(useTranscriptStore.getState().rows.first?.status).toBe("running");
    expect(useTranscriptStore.getState().rows.second?.status).toBe("queued");
  });

  it("keeps a completed transcript current throughout a failed replacement", async () => {
    mockCommands({ transcribe: () => null });
    useTranscriptStore.setState({
      rows: {
        video: {
          status: "ready",
          text: "previous words",
          message: null,
          percent: null,
          replacement: null,
        },
      },
    });

    await useTranscriptStore.getState().start("video", true);
    expect(invokeCalls).toContainEqual({
      command: "transcribe",
      args: { hash: "video", replace: true },
    });
    expect(useTranscriptStore.getState().rows.video).toMatchObject({
      status: "ready",
      text: "previous words",
      replacement: { status: "queued" },
    });

    fireEvent("transcribe://progress", { hash: "video", percent: 40 });
    expect(useTranscriptStore.getState().rows.video).toMatchObject({
      status: "ready",
      text: "previous words",
      replacement: { status: "running", percent: 40 },
    });

    fireEvent("transcribe://error", {
      hash: "video",
      message: "model stopped",
    });
    expect(useTranscriptStore.getState().rows.video).toMatchObject({
      status: "ready",
      text: "previous words",
      replacement: { status: "failed", message: "model stopped" },
    });
  });
});
