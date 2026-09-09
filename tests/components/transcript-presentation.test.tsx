// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import MetadataPane from "../../src/components/MetadataPane";
import TranscriptBlock from "../../src/components/TranscriptBlock";
import type { ItemDetail, ItemWorkState } from "../../src/models/items";
import { useContentSessionStore } from "../../src/state/content-session-store";
import { useTranscriptStore } from "../../src/state/transcript-store";
import { emitCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";
import { viewerOwnsKey } from "../../src/utils/viewerKeys";

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
  useContentSessionStore.setState({
    transcriptOpen: { video: false, audio: false },
    transcriptViews: { interview: { scrollTop: 120, selection: null } },
  });
});
afterEach(cleanup);

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
    expect(viewerOwnsKey(event("PageDown"), "video", detail.fileName)).toBe(false);
    for (const key of [" ", "f", "Escape", "Delete", "ArrowRight"]) {
      expect(viewerOwnsKey(event(key), "video", detail.fileName)).toBe(true);
    }
  });

  it.each(["running", "failed"] as const)("keeps a completed transcript readable during %s replacement work", async (state) => {
    const work: ItemWorkState = { state, hasValue: true, reason: null, done: 5, total: 100 };
    render(<TranscriptBlock hash="interview" medium="video" variant="details" work={work} />);
    expect(await screen.findByText("Final line")).toBeTruthy();
    expect(screen.getByText(state === "running" ? /Updating transcript/ : /The replacement failed/)).toBeTruthy();
  });
});
