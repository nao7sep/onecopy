// @vitest-environment happy-dom
import { EMPTY_ITEM_WORK } from "../helpers/itemWork";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it } from "vitest";
import PreviewWindow from "../../src/windows/PreviewWindow";
import FullscreenViewWindow from "../../src/windows/FullscreenViewWindow";
import PreviewSurface from "../../src/components/PreviewSurface";
import { useTranscriptStore } from "../../src/state/transcript-store";
import { useContentSessionStore } from "../../src/state/content-session-store";

import { emitCalls, fireEvent as deliver, mockCommands, resetTauriMocks, invokeCalls } from "../mocks/tauri";

const detail = {
  fileName: "interview.m4a", kind: "other", byteSize: 100, width: null, height: null,
  durationMs: 30_000, dateState: "undated" as const, resolvedUtcMs: null,
  resolvedSource: null, dateOnly: false, copyPaths: [], copyCount: 1,
  companionPaths: [], stripFrames: null,
};

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useTranscriptStore.setState({ rows: {} });
  useContentSessionStore.setState({ transcriptOpen: { video: true, audio: true }, transcriptViews: {} });
  mockCommands({
    binaries_state: () => [],
    transcript_get: () => ({ status: "ready", text: "[0:01] Original words", message: null }),
    transcribe: () => null,
    log_event: () => null,
    logging_debug_enabled: () => false,
  });
});
afterEach(cleanup);

it.each(["preview", "fullscreen"])("keeps %s transcripts live with reading controls and a route to Main", async (surface) => {
  render(surface === "preview" ? <PreviewWindow /> : <FullscreenViewWindow />);
  await act(async () => {});
  await act(async () => {
    if (surface === "preview") deliver("preview://show", { hash: "interview", pathId: null, detail, selectionRevision: 8 });
    else deliver("fullscreen-view://state", {
      token: "session", item: { ...detail, hash: "interview", pathId: 1, derivedWork: EMPTY_ITEM_WORK },
      detail, index: 0, length: 1, pendingDelete: null, sectionKind: "other", failure: null,
    });
  });
  expect(screen.getByText("Original words")).toBeTruthy();
  expect(screen.getByRole("button", { name: "0:01" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: /Re-transcribe|Transcribe this file|Cancel update|Retry/ })).toBeNull();
  await act(async () => deliver("transcribe://progress", { hash: "interview", percent: 25, replacement: true }));
  expect(screen.getByText("Original words")).toBeTruthy();
  expect(screen.queryByRole("button", { name: /Cancel/ })).toBeNull();
  await act(async () => deliver("transcribe://done", { hash: "interview", text: "[0:02] Updated words" }));
  expect(screen.getByText("Updated words")).toBeTruthy();
  expect(screen.queryByText("Original words")).toBeNull();
  await act(async () => fireEvent.click(screen.getByRole("button", { name: "Background work & tools" })));
  expect(emitCalls.some((call) => call.event === "viewing://open-background-work")).toBe(true);
  expect(invokeCalls.some((call) => call.command === "transcribe")).toBe(false);
});

it("retains transcription management in Main's embedded preview", async () => {
  render(<PreviewSurface hash="interview" detail={detail} surface="preview-split" />);
  await act(async () => {});
  await act(async () => fireEvent.click(screen.getByRole("button", { name: "Re-transcribe" })));
  expect(invokeCalls).toContainEqual({ command: "transcribe", args: { hash: "interview", replace: true } });
});
