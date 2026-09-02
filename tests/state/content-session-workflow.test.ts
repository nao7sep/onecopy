import { beforeEach, describe, expect, it } from "vitest";
import type { ContentSessionState } from "../../src/models/contentSession";
import { installContentSessionWorkflow } from "../../src/workflows/content-session";
import { setTextWrap } from "../../src/state/content-session-store";
import { emit, emitCalls, fireEvent, resetTauriMocks } from "../mocks/tauri";

function latestState(): ContentSessionState {
  const call = [...emitCalls]
    .reverse()
    .find((entry) => entry.event === "content-session://state");
  return call?.payload as ContentSessionState;
}

beforeEach(async () => {
  resetTauriMocks({ keepListeners: true });
  await installContentSessionWorkflow();
});

describe("content session workflow", () => {
  it("returns the original transport rejection to the affected control", async () => {
    const cause = new Error("EACCES /private/tmp/session IPC sentinel");
    emit.mockRejectedValueOnce(cause);

    await expect(setTextWrap(false)).rejects.toBe(cause);
  });

  it("shares text and transcript presentation choices across webviews for one app run", () => {
    fireEvent("content-session://set-text-wrap", { wrap: false });
    fireEvent("content-session://set-text-encoding", {
      key: "content-hash",
      encoding: "shift_jis",
    });
    fireEvent("content-session://set-transcript-open", {
      medium: "video",
      open: true,
    });

    expect(latestState()).toEqual({
      textWrap: false,
      textEncodings: { "content-hash": "shift_jis" },
      transcriptOpen: { video: true, audio: true },
      transcriptViews: {},
    });
  });

  it("retains transcript position across a same-item handoff and clears it on item exit", () => {
    fireEvent("playback://state", {
      key: "clip",
      medium: "video",
      owner: "preview-split",
      position: 2,
      playing: true,
      soundEnabled: true,
      volume: 1,
    });
    fireEvent("content-session://set-transcript-view", {
      key: "clip",
      view: { scrollTop: 80, selection: [4, 9] },
    });
    fireEvent("playback://state", {
      key: "clip",
      medium: "video",
      owner: null,
      position: 2,
      playing: true,
      soundEnabled: true,
      volume: 1,
    });
    expect(latestState().transcriptViews.clip).toEqual({
      scrollTop: 80,
      selection: [4, 9],
    });

    fireEvent("playback://state", null);
    expect(latestState().transcriptViews).toEqual({});
  });
});
