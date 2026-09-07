import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import {
  installTranscriptEventWiring,
  useTranscriptStore,
} from "../../src/state/transcript-store";
import {
  fireEvent,
  invokeCalls,
  mockCommands,
  resetTauriMocks,
} from "../mocks/tauri";

beforeAll(async () => {
  await installTranscriptEventWiring();
});

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  fireEvent("transcribe://cancelled", { hash: "background-video" });
  useTranscriptStore.setState({ rows: {} });
  mockCommands({
    transcript_get: () => ({ status: "pending", text: null, message: null }),
  });
});

describe("transcript projection", () => {
  it("keeps a newer completion when an older receipt load fails", async () => {
    let rejectLoad: ((error: Error) => void) | undefined;
    mockCommands({
      transcript_get: () =>
        new Promise((_resolve, reject) => {
          rejectLoad = reject;
        }),
    });

    const loading = useTranscriptStore.getState().load("video");
    fireEvent("transcribe://done", { hash: "video", text: "finished" });
    rejectLoad?.(new Error("older receipt unavailable"));
    await loading;

    expect(useTranscriptStore.getState().rows.video).toMatchObject({
      status: "ready",
      text: "finished",
      message: null,
    });
  });

  it("keeps a newer completion when the older start receipt rejects", async () => {
    let rejectStart: ((error: Error) => void) | undefined;
    mockCommands({
      transcribe: () =>
        new Promise((_resolve, reject) => {
          rejectStart = reject;
        }),
    });

    const starting = useTranscriptStore.getState().start("video");
    fireEvent("transcribe://done", { hash: "video", text: "finished" });
    rejectStart?.(new Error("older start reply lost"));
    await starting;

    expect(useTranscriptStore.getState().rows.video).toMatchObject({
      status: "ready",
      text: "finished",
      message: null,
    });
  });

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
      replacement: {
        status: "failed",
        message:
          "Transcription could not finish. Check the media file and managed tools, then try again.",
      },
    });
  });
});
