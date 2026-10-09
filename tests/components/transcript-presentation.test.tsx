// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import MetadataPane from "../../src/components/MetadataPane";
import TranscriptBlock from "../../src/components/TranscriptBlock";
import { EMPTY_ITEM_WORK, type ItemDetail, type ItemWorkState, type SectionItem } from "../../src/models/items";
import { useContentSessionStore } from "../../src/state/content-session-store";
import { useTranscriptStore } from "../../src/state/transcript-store";
import { useDerivedWorkStore } from "../../src/state/derived-work-store";
import { useAppShellStore } from "../../src/state/app-shell-store";
import { emitCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";
import { fullscreenViewOwnsKey } from "../../src/utils/viewerKeys";

const detail: ItemDetail = {
  fileName: "interview.mp4", kind: "video", byteSize: 1000,
  width: 1920, height: 1080, durationMs: 10000,
  dateState: "dated", resolvedUtcMs: 0, resolvedSource: "metadata", dateOnly: false,
  copyPaths: ["/fixture/interview.mp4"], companionPaths: ["/fixture/interview.xmp"],
  stripFrames: null,
};

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    transcript_get: () => ({ status: "ready", text: "[0:01] First line\n[0:02] Final line", message: null }),
    log_event: () => null,
  });
  useTranscriptStore.setState({ rows: {} });
  useDerivedWorkStore.setState({ runtime: null, snapshot: null });
  useContentSessionStore.setState({
    transcriptOpen: { video: false, audio: false },
    transcriptViews: { interview: { scrollTop: 120, selection: null } },
  });
});
afterEach(cleanup);

describe("Details no-detail states (R5.1 D13)", () => {
  const anchorItem: SectionItem = {
    hash: "interview", pathId: 1, fileName: "interview.mp4", resolvedUtcMs: null, copyCount: 1,
    width: 1920, height: 1080, hasThumb: true, similarGroupId: null, sharpness: null,
    faceScore: null, byteSize: 1000, hasCompanions: false, durationMs: 10000,
    dirPaths: ["/fixture"], derivedWork: EMPTY_ITEM_WORK,
  };

  it("shows truthful loading rather than \"No selection\" while an anchor's detail is still loading", () => {
    render(<MetadataPane detail={null} hash="interview" item={anchorItem} />);
    expect(screen.getByText("Loading…")).toBeTruthy();
    expect(screen.queryByText("No selection")).toBeNull();
  });

  it("shows \"No selection\" only when there truly is no anchor", () => {
    render(<MetadataPane detail={null} hash={null} item={null} />);
    expect(screen.getByText("No selection")).toBeTruthy();
  });
});

describe("transcript presentation owners", () => {
  it("puts the entire expanded Details transcript last without changing Preview reading state", async () => {
    const view = render(<MetadataPane detail={detail} hash="interview" item={null} />);
    const final = await screen.findByText("Final line");
    const panel = final.closest("section")!;
    expect(view.container.firstElementChild?.lastElementChild).toBe(panel);
    expect(screen.getByText("Companions (1)").compareDocumentPosition(panel) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(panel.className).toContain("border-t");
    expect(panel.className).not.toContain("overflow");
    expect(panel.className).not.toContain("max-h");
    expect(final.closest("ol")?.className).not.toContain("overflow");
    expect(screen.queryByRole("button", { name: /Expand|Collapse/ })).toBeNull();
    expect(screen.getByText("Date")).toBeTruthy();
    expect(screen.queryByText("Taken")).toBeNull();
    fireEvent.scroll(panel);
    fireEvent.mouseUp(final);
    fireEvent.keyUp(final);
    await act(async () => {});
    expect(emitCalls.filter((call) => call.event.startsWith("content-session://"))).toEqual([]);
    expect(useContentSessionStore.getState().transcriptOpen.video).toBe(false);
    expect(useContentSessionStore.getState().transcriptViews.interview.scrollTop).toBe(120);
  });

  it("uses a single bounded Preview scroller with first-baseline timestamp rows", async () => {
    useContentSessionStore.setState({ transcriptOpen: { video: true, audio: false } });
    render(<TranscriptBlock hash="interview" medium="video" />);
    const text = await screen.findByText("Final line");
    const panel = text.closest("section")!;
    expect(panel.className).toContain("max-h-[35%]");
    expect(panel.className).toContain("overflow-y-auto");
    expect(text.closest("ol")?.className).not.toContain("overflow");
    expect(text.closest("li")?.className).toContain("items-baseline");
    expect(screen.getByRole("button", { name: "Collapse" })).toBeTruthy();
    expect(panel.scrollTop).toBe(120);
    fireEvent.keyDown(panel, { key: "ArrowDown" });
    expect(panel.scrollTop).toBe(160);
    const event = (key: string) => {
      const value = new KeyboardEvent("keydown", { key });
      Object.defineProperty(value, "target", { value: panel });
      return value;
    };
    expect(fullscreenViewOwnsKey(event("PageDown"), "video", detail.fileName)).toBe(false);
    for (const key of [" ", "Escape", "Delete", "ArrowRight"]) {
      expect(fullscreenViewOwnsKey(event(key), "video", detail.fileName)).toBe(true);
    }
  });

  // A cancelled re-transcription is reported,
  // not silently reverted, and its own actions (Cancel/Background Work) no
  // longer apply once nothing is in progress.
  it("reports a cancelled replacement and offers only Re-transcribe", async () => {
    render(<TranscriptBlock hash="interview" medium="video" />);
    fireEvent.click(screen.getByRole("button", { name: "Expand" }));
    await act(async () => {
      useContentSessionStore.setState({
        transcriptOpen: { video: true, audio: false },
      });
    });
    await screen.findByText("Final line");
    act(() => {
      useTranscriptStore.setState({
        rows: {
          interview: {
            status: "ready",
            text: "[0:01] First line\n[0:02] Final line",
            message: null,
            percent: null,
            replacement: { status: "cancelled", message: null, percent: null },
          },
        },
      });
    });
    expect(await screen.findByText(/The replacement was cancelled/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Cancel update" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Background Work" })).toBeNull();
    expect(screen.getByRole("button", { name: "Re-transcribe" })).toBeTruthy();
  });

  it.each(["running", "failed"] as const)("keeps a completed transcript readable during %s replacement work", async (state) => {
    const work: ItemWorkState = { state, hasValue: true, reason: null, done: 5, total: 100 };
    render(<TranscriptBlock hash="interview" medium="video" variant="details" work={work} />);
    expect(await screen.findByText("Final line")).toBeTruthy();
    expect(screen.getByText(state === "running" ? /Updating transcript/ : /The replacement failed/)).toBeTruthy();
  });

  it("shows runtime pause state even when the background window has never opened", async () => {
    mockCommands({ transcript_get: () => ({ status: "pending", text: null, message: null }) });
    useDerivedWorkStore.setState({ snapshot: null, runtime: { workerRunning: true, pausedClasses: ["video-transcripts"], active: null } });
    useContentSessionStore.setState({ transcriptOpen: { video: true, audio: false } });
    render(<TranscriptBlock hash="paused" medium="video" work={{ state: "pending", hasValue: false, reason: null, done: null, total: null }} />);
    expect(await screen.findByText("Queued — transcription is paused.")).toBeTruthy();
  });

  it.each([
    ["waiting-for-transcription-model", "Background work & tools", "backgroundWork"],
    ["unsupported-acceleration", "Settings", "settings"],
  ] as const)("offers the remedy for %s where it is resolved", async (reason, label, surface) => {
    mockCommands({ transcript_get: () => ({ status: "pending", text: null, message: null }), log_event: () => null });
    useAppShellStore.setState({ utilitySurface: null });
    const work: ItemWorkState = { state: "unavailable", hasValue: false, reason, done: null, total: null };
    useContentSessionStore.setState({ transcriptOpen: { video: true, audio: false } });
    render(<TranscriptBlock hash="clip" medium="video" work={work} />);
    fireEvent.click(await screen.findByRole("button", { name: label }));
    expect(useAppShellStore.getState().utilitySurface).toBe(surface);
  });
});
