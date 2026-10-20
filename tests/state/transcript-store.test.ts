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
import { inEnglish } from "../helpers/i18n";
import { message } from "../../src/i18n/translate";
import { CORE_NAMES } from "../helpers/coreNames";

beforeAll(async () => {
  await installTranscriptEventWiring();
});

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  fireEvent(CORE_NAMES.transcribeEvents.cancelled, {
    hash: "background-video",
    replacement: false,
  });
  useTranscriptStore.setState({ rows: {} });
  mockCommands({
    transcript_get: () => ({ status: CORE_NAMES.transcriptStatus.pending, text: null, message: null }),
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
    fireEvent(CORE_NAMES.transcribeEvents.done, {
      hash: "video",
      text: "finished",
      replacement: false,
    });
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
    fireEvent(CORE_NAMES.transcribeEvents.done, {
      hash: "video",
      text: "finished",
      replacement: false,
    });
    rejectStart?.(new Error("older start reply lost"));
    await starting;

    expect(useTranscriptStore.getState().rows.video).toMatchObject({
      status: "ready",
      text: "finished",
      message: null,
    });
  });

  it("tracks unseen automatic work without caching every library item", async () => {
    fireEvent(CORE_NAMES.transcribeEvents.progress, {
      hash: "background-video",
      percent: 23,
      replacement: false,
    });
    expect(useTranscriptStore.getState().rows).toEqual({});

    await useTranscriptStore.getState().load("background-video");
    expect(
      useTranscriptStore.getState().rows["background-video"],
    ).toMatchObject({
      status: "running",
      percent: 23,
    });

    fireEvent(CORE_NAMES.transcribeEvents.done, {
      hash: "background-video",
      text: "hello",
      replacement: false,
    });
    expect(
      useTranscriptStore.getState().rows["background-video"],
    ).toMatchObject({
      status: "ready",
      text: "hello",
    });
  });

  it("keeps a second manual request queued until its own progress begins", async () => {
    mockCommands({
      transcript_get: () => ({ status: CORE_NAMES.transcriptStatus.pending, text: null, message: null }),
      transcribe: () => null,
    });

    await useTranscriptStore.getState().start("first");
    await useTranscriptStore.getState().start("second");

    expect(useTranscriptStore.getState().rows.first?.status).toBe("queued");
    expect(useTranscriptStore.getState().rows.second?.status).toBe("queued");

    fireEvent(CORE_NAMES.transcribeEvents.progress, {
      hash: "first",
      percent: 0,
      replacement: false,
    });
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

    fireEvent(CORE_NAMES.transcribeEvents.progress, {
      hash: "video",
      percent: 40,
      replacement: true,
    });
    expect(useTranscriptStore.getState().rows.video).toMatchObject({
      status: "ready",
      text: "previous words",
      replacement: { status: "running", percent: 40 },
    });

    fireEvent(CORE_NAMES.transcribeEvents.error, {
      hash: "video",
      message: "model stopped",
      replacement: true,
    });
    expect(useTranscriptStore.getState().rows.video).toMatchObject({
      status: "ready",
      text: "previous words",
      replacement: { status: "failed" },
    });
    expect(inEnglish(useTranscriptStore.getState().rows.video?.replacement?.message)).toBe(
      "Transcription could not finish. Check the media file and managed tools, then try again.",
    );
  });

  it("classifies replacement events from another window without hiding the completed transcript", async () => {
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

    fireEvent(CORE_NAMES.transcribeEvents.progress, {
      hash: "video",
      percent: 25,
      replacement: true,
    });
    expect(useTranscriptStore.getState().rows.video).toMatchObject({
      status: "ready",
      text: "previous words",
      replacement: { status: "running", percent: 25 },
    });

    fireEvent(CORE_NAMES.transcribeEvents.error, {
      hash: "video",
      message: "model stopped",
      replacement: true,
    });
    expect(useTranscriptStore.getState().rows.video).toMatchObject({
      status: "ready",
      text: "previous words",
      replacement: { status: "failed" },
    });

    fireEvent(CORE_NAMES.transcribeEvents.progress, {
      hash: "video",
      percent: 50,
      replacement: true,
    });

    fireEvent(CORE_NAMES.transcribeEvents.cancelled, { hash: "video", replacement: true });
    // Cancellation reports the new attempt
    // rather than silently reverting to no replacement in progress.
    expect(useTranscriptStore.getState().rows.video).toMatchObject({
      status: "ready",
      text: "previous words",
      replacement: { status: "cancelled" },
    });
  });

  it("loads the completed receipt as replacement work when progress arrived first", async () => {
    mockCommands({
      transcript_get: () => ({
        status: CORE_NAMES.transcriptStatus.ready,
        text: "previous words",
        message: null,
      }),
    });
    fireEvent(CORE_NAMES.transcribeEvents.progress, {
      hash: "video",
      percent: 40,
      replacement: true,
    });

    await useTranscriptStore.getState().load("video");

    expect(useTranscriptStore.getState().rows.video).toMatchObject({
      status: "ready",
      text: "previous words",
      replacement: { status: "running", percent: 40 },
    });
  });
});

describe("transcript receipts the core reports", () => {
  it("shows a ready receipt's text, a failed one's sentence and a pending one as still owed", async () => {
    const { pending, ready, failed } = CORE_NAMES.transcriptStatus;
    const receipts: Record<string, { status: string; text: string | null; message: string | null }> = {
      "owed": { status: pending, text: null, message: null },
      "done": { status: ready, text: "[0:01] words", message: null },
      "broken": { status: failed, text: null, message: "decoder exited" },
    };
    mockCommands({
      transcript_get: (args) => receipts[(args as { hash: string }).hash],
    });
    for (const hash of Object.keys(receipts)) await useTranscriptStore.getState().load(hash);

    const rows = useTranscriptStore.getState().rows;
    expect(rows.owed).toMatchObject({ status: "pending", text: null, message: null });
    expect(rows.done).toMatchObject({ status: "ready", text: "[0:01] words", message: null });
    expect(rows.broken).toMatchObject({ status: "failed" });
    expect(inEnglish(rows.broken?.message)).toBe(inEnglish(message("transcript.couldNotFinish")));
  });
});
