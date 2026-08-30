// @vitest-environment happy-dom

import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import PreviewSurface from "../../src/components/PreviewSurface";
import { useAppStore } from "../../src/state/app-store";
import { useBinariesStore } from "../../src/state/binaries-store";
import { useTranscriptStore } from "../../src/state/transcript-store";
import { useContentSessionStore } from "../../src/state/content-session-store";
import {
  emitCalls,
  fireEvent as fireTauriEvent,
  mockCommands,
  resetTauriMocks,
} from "../mocks/tauri";

const DETAIL = {
  fileName: "family.mov",
  kind: "video",
  byteSize: 1_000,
  width: 1920,
  height: 1080,
  durationMs: 30_000,
  dateState: "dated" as const,
  resolvedUtcMs: 0,
  resolvedSource: "metadata",
  dateOnly: false,
  copyPaths: ["/videos/family.mov"],
  companionPaths: [],
  stripFrames: 4,
};

const AUDIO_DETAIL = {
  ...DETAIL,
  fileName: "interview.m4a",
  kind: "audio",
  width: null,
  height: null,
  durationMs: 30_000,
  stripFrames: null,
};

const IMAGE_DETAIL = {
  ...DETAIL,
  fileName: "family.jpg",
  kind: "image",
  durationMs: null,
  stripFrames: null,
};

const OTHER_DETAIL = {
  ...DETAIL,
  fileName: "notes.txt",
  kind: "other",
  width: null,
  height: null,
  durationMs: null,
  stripFrames: null,
};

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    transcript_get: () => ({
      status: "ready",
      text: "[0:01] hello",
      message: null,
    }),
    open_item_externally: () => null,
    text_preview: () => ({
      body: "text",
      text: "first line\nsecond line",
      encoding: "utf-8",
      contentKey: "text-content-hash",
      encodings: ["utf-8", "shift_jis"],
      byteSize: 22,
    }),
    background_work_snapshot: () => ({
      masterPaused: false,
      classes: [],
      activeItem: null,
    }),
    log_event: () => null,
  });
  useTranscriptStore.setState({ rows: {} });
  useContentSessionStore.setState({
    textWrap: true,
    textEncodings: {},
    transcriptOpen: { video: false, audio: true },
    transcriptViews: {},
  });
  useBinariesStore.setState({ entries: [] });
  useAppStore.setState({
    appData: {
      config: {
        videoAutoplay: false,
        audioAutoplay: false,
        soundEnabled: true,
        playbackVolume: 1,
      },
      state: null,
      dataRoot: "/app",
      debugEnabled: false,
      quarantines: [],
    },
  });
  vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  useAppStore.setState({ appData: null });
  cleanup();
});

describe("shared video presentation", () => {
  it("registers one named playback surface for central ownership", async () => {
    render(
      <PreviewSurface surface="quick" hash="video-hash" detail={DETAIL} />,
    );
    await act(async () => {});
    expect(emitCalls).toContainEqual({
      event: "playback://register",
      payload: { surface: "quick", key: "video-hash", medium: "video" },
    });
  });

  it("plays only when the central session assigns this surface", async () => {
    render(
      <PreviewSurface surface="quick" hash="video-hash" detail={DETAIL} />,
    );
    await act(async () => {});

    await act(async () => {
      fireTauriEvent("playback://state", {
        key: "video-hash",
        medium: "video",
        owner: "quick",
        position: 4,
        playing: true,
        soundEnabled: false,
        volume: 0.5,
      });
    });

    const video = document.querySelector("video")!;
    fireEvent(video, new Event("loadedmetadata"));
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalledOnce();
    expect(video.muted).toBe(true);
    expect(video.volume).toBe(0.5);
  });

  it("overlays timestamped snapshots, seeks and plays, and keeps transcript below", async () => {
    const view = render(
      <PreviewSurface
        surface="quick"
        hash="video-hash"
        detail={DETAIL}
        keyboardActive
      />,
    );
    const video = view.container.querySelector("video");
    expect(video).not.toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Expand" }));
    await act(async () => {
      fireTauriEvent("content-session://state", {
        textWrap: true,
        textEncodings: {},
        transcriptOpen: { video: true, audio: true },
        transcriptViews: {},
      });
    });
    expect(await screen.findByRole("button", { name: "0:01" })).toBeTruthy();
    expect(screen.getByText("hello")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Managed tools" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Play from 0:12" }));

    expect(emitCalls).toContainEqual({
      event: "playback://seek",
      payload: { key: "video-hash", position: 12, play: true },
    });
    expect(
      screen.getByRole("button", { name: /Open in player/i }),
    ).toBeTruthy();

    fireEvent.keyDown(window, { key: " " });
    expect(emitCalls.some((call) => call.event === "playback://toggle")).toBe(
      false,
    );
  });

  it("pauses a playing video during held inspection and resumes it on release", () => {
    vi.useFakeTimers();
    const view = render(
      <PreviewSurface
        surface="preview-split"
        hash="video-hash"
        detail={DETAIL}
      />,
    );
    const video = view.container.querySelector("video")!;
    let paused = false;
    Object.defineProperty(video, "paused", {
      configurable: true,
      get: () => paused,
    });
    const pause = vi.spyOn(video, "pause").mockImplementation(() => {
      paused = true;
    });
    const play = vi.spyOn(video, "play").mockImplementation(() => {
      paused = false;
      return Promise.resolve();
    });
    const viewport = screen.getByTitle(
      "Press and hold the picture for original pixels",
    );

    fireEvent.pointerDown(viewport, {
      pointerId: 8,
      button: 0,
      isPrimary: true,
      clientX: 20,
      clientY: 20,
    });
    act(() => vi.advanceTimersByTime(135));

    expect(pause).toHaveBeenCalledOnce();
    expect(screen.queryByRole("button", { name: "Play" })).toBeNull();

    fireEvent.pointerUp(window, { pointerId: 8, clientX: 40, clientY: 30 });

    expect(play).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "Play" })).toBeTruthy();
    vi.useRealTimers();
  });

  it("shows audio playback and the shared content-owned transcript", async () => {
    const view = render(
      <PreviewSurface
        surface="preview-split"
        hash="audio-hash"
        detail={AUDIO_DETAIL}
      />,
    );

    expect(view.container.querySelector("audio")).not.toBeNull();
    expect(await screen.findByRole("button", { name: "0:01" })).toBeTruthy();
    expect(screen.getByText("hello")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Re-transcribe" })).toBeTruthy();
  });

  it("shows bounded read-only text with session encoding and wrapping controls", async () => {
    render(
      <PreviewSurface
        surface="quick"
        hash={null}
        pathId={8}
        detail={OTHER_DETAIL}
      />,
    );

    expect(await screen.findByText(/first line/)).toBeTruthy();
    expect((screen.getByRole("combobox") as HTMLSelectElement).value).toBe(
      "automatic",
    );
    fireEvent.change(screen.getByRole("combobox"), {
      target: { value: "shift_jis" },
    });
    expect(emitCalls).toContainEqual({
      event: "content-session://set-text-encoding",
      payload: { key: "text-content-hash", encoding: "shift_jis" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Wrap on" }));
    expect(emitCalls).toContainEqual({
      event: "content-session://set-text-wrap",
      payload: { wrap: false },
    });
  });

  it("shows complete attributes when bounded content is binary", async () => {
    mockCommands({
      text_preview: () => ({
        body: "attributes",
        reason: "The file looks binary rather than textual.",
        byteSize: 1_000,
      }),
    });

    render(
      <PreviewSurface
        surface="quick"
        hash={null}
        pathId={8}
        detail={OTHER_DETAIL}
      />,
    );

    expect(
      await screen.findByText("The file looks binary rather than textual."),
    ).toBeTruthy();
    expect(screen.getByText("1 exact copy")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: /Reveal \/videos\/family.mov/ }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Open in default app" }),
    ).toBeTruthy();
  });

  it("keeps alternative encodings available after automatic decoding fails", async () => {
    mockCommands({
      text_preview: () => ({
        body: "decodeError",
        reason: "The bytes are not valid UTF-8 text.",
        contentKey: "unknown-text",
        encodings: ["utf-8", "shift_jis"],
        byteSize: 20,
      }),
    });

    render(
      <PreviewSurface
        surface="quick"
        hash={null}
        pathId={8}
        detail={OTHER_DETAIL}
      />,
    );

    expect(
      await screen.findByText("The bytes are not valid UTF-8 text."),
    ).toBeTruthy();
    expect(screen.getByRole("option", { name: /shift_jis/ })).toBeTruthy();
  });

  it("falls back truthfully after specialized image decoding fails", async () => {
    mockCommands({
      ensure_preview: () => Promise.reject(new Error("decoder unavailable")),
    });
    render(
      <PreviewSurface
        surface="quick"
        hash="image-hash"
        detail={IMAGE_DETAIL}
      />,
    );

    fireEvent.error(screen.getByAltText("family.jpg"));

    expect(
      await screen.findByText(/Built-in image preview failed/),
    ).toBeTruthy();
    expect(screen.getByText(/first line/)).toBeTruthy();
  });

  it("falls back truthfully after specialized audio playback fails", async () => {
    const view = render(
      <PreviewSurface
        surface="quick"
        hash="audio-hash"
        detail={AUDIO_DETAIL}
      />,
    );

    fireEvent.error(view.container.querySelector("audio")!);

    expect(
      await screen.findByText("Built-in audio playback failed."),
    ).toBeTruthy();
    expect(screen.getByText(/first line/)).toBeTruthy();
  });
});
