// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { seedAppConfig } from "../helpers/config";
import { emitCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";
import TranscriptBlock from "../../src/components/TranscriptBlock";
import TranscriptRows from "../../src/components/TranscriptRows";
import { parseTranscript } from "../../src/models/transcript";
import { useContentSessionStore } from "../../src/state/content-session-store";
import { usePlaybackClientStore } from "../../src/state/playback-client-store";
import { useTranscriptStore } from "../../src/state/transcript-store";

vi.mock("../../src/models/transcript", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../src/models/transcript")>();
  return { ...actual, parseTranscript: vi.fn(actual.parseTranscript) };
});
vi.mock("../../src/components/TranscriptRows", async (importOriginal) => {
  const { default: Rows } = await importOriginal<typeof import("../../src/components/TranscriptRows")>();
  return { default: vi.fn((props: React.ComponentProps<typeof Rows>) => <Rows {...props} />) };
});

beforeEach(() => {
  vi.clearAllMocks();
  resetTauriMocks({ keepListeners: true });
  seedAppConfig();
  mockCommands({
    transcript_get: () => ({ status: "ready", text: "[0:01] First line\n[0:02] Final line", message: null }),
    log_event: () => null,
  });
  useTranscriptStore.setState({ rows: {} });
  usePlaybackClientStore.setState({ session: null });
  useContentSessionStore.setState({
    transcriptOpen: { video: false, audio: false },
    transcriptViews: { interview: { scrollTop: 120, selection: null } },
  });
});
afterEach(cleanup);

it("parses unchanged text once, builds only visible rows and isolates playback highlighting", async () => {
  const view = render(<TranscriptBlock hash="interview" medium="video" />);
  await waitFor(() => expect(useTranscriptStore.getState().rows.interview?.status).toBe("ready"));
  const tick = (position: number) => act(() => usePlaybackClientStore.setState({ session: {
    key: "interview", medium: "video", owner: "preview-window", position,
    playing: true, soundEnabled: true, volume: 1,
  } }));
  for (const position of [0, 1, 1.2, 1.5]) tick(position);
  expect(parseTranscript).toHaveBeenCalledTimes(1);
  expect(TranscriptRows).not.toHaveBeenCalled();
  expect(view.container.querySelector("ol")).toBeNull();

  act(() => useContentSessionStore.setState({ transcriptOpen: { video: true, audio: false } }));
  const first = screen.getByText("First line");
  const final = screen.getByText("Final line");
  const panel = first.closest("section")!;
  expect(panel.scrollTop).toBe(120);
  const range = document.createRange();
  range.setStart(first.firstChild!, 0);
  range.setEnd(first.firstChild!, 5);
  const selection = window.getSelection()!;
  selection.removeAllRanges();
  selection.addRange(range);
  expect(first.closest("li")?.className).toContain("bg-primary-surface");
  for (const position of [1.6, 1.9, 2, 2.1]) tick(position);
  expect(first.closest("li")?.className).not.toContain("bg-primary-surface");
  expect(final.closest("li")?.className).toContain("bg-primary-surface");
  expect(selection.toString()).toBe("First");
  expect(screen.getByText("First line")).toBe(first);
  expect(panel.scrollTop).toBe(120);
  expect(TranscriptRows).toHaveBeenCalledTimes(1);
  expect(parseTranscript).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole("button", { name: "0:02" }));
  expect(emitCalls).toContainEqual({ event: "playback://seek", payload: { key: "interview", position: 2, play: true } });

  act(() => useContentSessionStore.setState({ transcriptOpen: { video: false, audio: false } }));
  tick(3);
  expect(TranscriptRows).toHaveBeenCalledTimes(1);
  act(() => useContentSessionStore.setState({ transcriptOpen: { video: true, audio: false } }));
  expect(parseTranscript).toHaveBeenCalledTimes(1);
  const current = useTranscriptStore.getState().rows.interview;
  act(() => useTranscriptStore.setState({ rows: { interview: { ...current, text: "[0:01] Replaced line" } } }));
  expect(screen.getByText("Replaced line")).toBeTruthy();
  expect(parseTranscript).toHaveBeenCalledTimes(2);
});
