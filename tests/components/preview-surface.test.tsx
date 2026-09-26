// @vitest-environment happy-dom

import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import PreviewSurface from "../../src/components/PreviewSurface";
import { useAppStore } from "../../src/state/app-store";
import { useBinariesStore } from "../../src/state/binaries-store";
import { useTranscriptStore } from "../../src/state/transcript-store";
import { useContentSessionStore } from "../../src/state/content-session-store";
import { useWindowPreferencesStore } from "../../src/state/window-preferences-store";
import {
  emitCalls,
  emit,
  fireEvent as fireTauriEvent,
  invokeCalls,
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
      workerRunning: true, pausedClasses: [],
      classes: [],
      activeItem: null,
    }),
    log_event: () => null,
    record_recent_notification: () => ({}),
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
  it.each([2, 3, 4])("shows one noticeable video failure for media error %s and keeps native diagnostics out of the UI", async (code) => {
    const view = render(<PreviewSurface surface="quick" hash="video-hash" detail={DETAIL} />);
    const video = view.container.querySelector("video")!;
    Object.defineProperty(video, "error", { value: { code, message: "fixture native decoder sentinel" } });
    fireEvent.error(video);
    const result = screen.getByRole("alert");
    expect(result.textContent).toContain(code === 2 ? "could not be read" : code === 3 ? "could not be decoded" : "could not be loaded");
    expect(screen.queryByText(/fixture native decoder sentinel/)).toBeNull();
    expect(screen.queryByText(/This codec/)).toBeNull();
    expect(screen.getByAltText("family.mov")).toBeTruthy();
    expect(await screen.findByRole("button", { name: "Open in default app" })).toBeTruthy();
    await act(async () => {});
    expect(invokeCalls.filter((call) => call.command === "record_recent_notification" &&
      (call.args?.request as { kind?: string })?.kind === "video-playback-failed")).toHaveLength(1);
    expect(invokeCalls.some((call) => call.command === "log_event" &&
      (call.args?.entry as { diagnostic?: string })?.diagnostic === "fixture native decoder sentinel")).toBe(true);
  });

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

  it("re-announces a live surface when the coordinator becomes ready without remounting it", async () => {
    render(
      <PreviewSurface surface="quick" hash="video-hash" detail={DETAIL} />,
    );
    await act(async () => {});
    emitCalls.length = 0;

    await act(async () => {
      fireTauriEvent("playback://coordinator-ready", {});
    });

    expect(emitCalls).toContainEqual({
      event: "playback://register",
      payload: { surface: "quick", key: "video-hash", medium: "video" },
    });
    expect(
      emitCalls.some((call) => call.event === "playback://unregister"),
    ).toBe(false);
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

  // content-presentation.md: "Picture click and Enter toggle" (R5.3 untested
  // contract) -- clicking the video body itself must request the shared
  // player toggle for THIS surface's key, the same as pressing Enter does.
  it("toggles playback when the video picture itself is clicked", async () => {
    render(<PreviewSurface surface="quick" hash="video-hash" detail={DETAIL} />);
    await act(async () => {});
    emitCalls.length = 0;

    const video = document.querySelector("video")!;
    fireEvent.click(video);

    expect(emitCalls).toContainEqual({
      event: "playback://toggle",
      payload: { key: "video-hash" },
    });
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
    expect(screen.getByRole("button", { name: "Open in player" }).className).toContain("left-2");
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
    const viewport = screen.getByTitle("Press and hold for original pixels");

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
    expect(view.container.querySelector("audio")?.className).toBe("w-full");
    expect(await screen.findByRole("button", { name: "0:01" })).toBeTruthy();
    expect(screen.getByText("hello")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Re-transcribe" })).toBeTruthy();
  });

  it("clears a retained transcript-position failure after the matching choice succeeds", async () => {
    mockCommands({ record_recent_notification: () => ({}) });
    render(
      <PreviewSurface
        surface="preview-split"
        hash="audio-hash"
        detail={AUDIO_DETAIL}
      />,
    );
    const transcript = (await screen.findByText("hello")).closest("ol")!;
    emit.mockRejectedValueOnce(
      new Error("EACCES /private/tmp/transcript IPC sentinel"),
    );

    fireEvent.scroll(transcript.closest("section")!);
    expect(
      await screen.findByText("Couldn’t retain the transcript position."),
    ).toBeTruthy();

    emit.mockResolvedValueOnce(undefined);
    fireEvent.keyUp(transcript);
    await act(async () => {});

    expect(
      screen.queryByText("Couldn’t retain the transcript position."),
    ).toBeNull();
  });

  it("does not let an older failed position write overwrite a newer success", async () => {
    let rejectOlder!: (error: unknown) => void;
    const older = new Promise<void>((_resolve, reject) => {
      rejectOlder = reject;
    });
    mockCommands({ record_recent_notification: () => ({}) });
    render(
      <PreviewSurface
        surface="preview-split"
        hash="audio-hash"
        detail={AUDIO_DETAIL}
      />,
    );
    const transcript = (await screen.findByText("hello")).closest("ol")!;
    emit.mockImplementationOnce(() => older);
    emit.mockResolvedValueOnce(undefined);

    fireEvent.scroll(transcript.closest("section")!);
    fireEvent.keyUp(transcript);
    await act(async () => {
      rejectOlder(new Error("stale transcript failure"));
    });

    expect(
      screen.queryByText("Couldn’t retain the transcript position."),
    ).toBeNull();
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
    await waitFor(() =>
      expect(emitCalls).toContainEqual({
        event: "content-session://set-text-encoding",
        payload: { key: "text-content-hash", encoding: "shift_jis" },
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Wrap on" }));
    await waitFor(() =>
      expect(emitCalls).toContainEqual({
        event: "content-session://set-text-wrap",
        payload: { wrap: false },
      }),
    );
  });

  // viewing-sessions.md D6: text scroll is shared session state, like
  // encoding and wrap, so a placement switch (pane <-> separate window,
  // modeled here as an unmount/remount of the same live session) keeps the
  // reading position instead of restarting at the top.
  it("keeps the text scroll position across a placement switch", async () => {
    const view = render(
      <PreviewSurface surface="quick" hash={null} pathId={8} detail={OTHER_DETAIL} />,
    );
    const pre = await screen.findByText(/first line/);
    Object.defineProperty(pre, "scrollTop", { configurable: true, writable: true, value: 0 });
    pre.scrollTop = 240;
    fireEvent.scroll(pre);
    await waitFor(() =>
      expect(emitCalls).toContainEqual({
        event: "content-session://set-transcript-view",
        payload: { key: "text:text-content-hash", view: { scrollTop: 240, selection: null } },
      }),
    );
    await act(async () => {
      fireTauriEvent("content-session://state", {
        textWrap: true,
        textEncodings: {},
        transcriptOpen: { video: false, audio: true },
        transcriptViews: { "text:text-content-hash": { scrollTop: 240, selection: null } },
      });
    });
    view.unmount();

    render(
      <PreviewSurface surface="preview-window" hash={null} pathId={8} detail={OTHER_DETAIL} />,
    );
    const restored = await screen.findByText(/first line/);
    expect(restored.scrollTop).toBe(240);
  });

  it("keeps a rejected session choice on the affected text preview and out of the copy", async () => {
    mockCommands({ record_recent_notification: () => ({}) });
    render(
      <PreviewSurface
        surface="quick"
        hash={null}
        pathId={8}
        detail={OTHER_DETAIL}
      />,
    );
    await screen.findByText(/first line/);
    emit.mockRejectedValueOnce(
      new Error("TypeError: EACCES /private/tmp/session IPC sentinel"),
    );

    fireEvent.click(screen.getByRole("button", { name: "Wrap on" }));

    expect(await screen.findByText("Couldn’t change text wrapping.")).toBeTruthy();
    expect(document.body.textContent).not.toContain("EACCES");
    expect(document.body.textContent).not.toContain("/private/tmp");
    expect(document.body.textContent).not.toContain("IPC sentinel");
    expect(
      invokeCalls.some((call) => call.command === "record_recent_notification"),
    ).toBe(true);
  });

  it("keeps independent text-session failures and clears only the chosen result", async () => {
    mockCommands({ record_recent_notification: () => ({}) });
    render(
      <PreviewSurface surface="quick" hash={null} pathId={8} detail={OTHER_DETAIL} />,
    );
    await screen.findByText(/first line/);

    emit.mockRejectedValueOnce(new Error("wrap failed"));
    fireEvent.click(screen.getByRole("button", { name: "Wrap on" }));
    expect(await screen.findByText("Couldn’t change text wrapping.")).toBeTruthy();

    emit.mockRejectedValueOnce(new Error("encoding failed"));
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "shift_jis" } });
    expect(await screen.findByText("Couldn’t change the text encoding.")).toBeTruthy();
    expect(screen.getByText("Couldn’t change text wrapping.")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Close encoding result" }));
    expect(screen.queryByText("Couldn’t change the text encoding.")).toBeNull();
    expect(screen.getByText("Couldn’t change text wrapping.")).toBeTruthy();
  });

  it("does not let an older transcript-visibility failure overwrite a newer success", async () => {
    let rejectOlder!: (error: unknown) => void;
    const older = new Promise<void>((_resolve, reject) => { rejectOlder = reject; });
    mockCommands({ record_recent_notification: () => ({}) });
    render(
      <PreviewSurface surface="preview-split" hash="audio-hash" detail={AUDIO_DETAIL} />,
    );
    await screen.findByRole("button", { name: "Collapse" });
    emit.mockImplementationOnce(() => older);
    emit.mockResolvedValueOnce(undefined);

    fireEvent.click(screen.getByRole("button", { name: "Collapse" }));
    fireEvent.click(screen.getByRole("button", { name: "Collapse" }));
    await act(async () => rejectOlder(new Error("stale visibility failure")));

    expect(screen.queryByText("Couldn’t change the transcript view.")).toBeNull();
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

  it("keeps rejected preview diagnostics out of the interface", async () => {
    const diagnostic =
      "TypeError: EACCES /private/tmp/onecopy-secret IPC sentinel";
    mockCommands({
      text_preview: () => Promise.reject(new Error(diagnostic)),
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
      await screen.findByText(
        "OneCopy couldn’t prepare this text preview. The original file was not changed.",
      ),
    ).toBeTruthy();
    expect(document.body.textContent).not.toContain("EACCES");
    expect(document.body.textContent).not.toContain("/private/tmp");
    expect(document.body.textContent).not.toContain("IPC sentinel");
  });

  it("keeps an external-open failure on the affected preview and out of live global notices", async () => {
    mockCommands({
      open_item_externally: () => Promise.reject(new Error("native open failed")),
      record_recent_notification: () => ({}),
    });

    render(
      <PreviewSurface
        surface="quick"
        hash="image-hash"
        detail={IMAGE_DETAIL}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: "Open in default app" }),
    );

    expect((await screen.findByRole("alert")).textContent).toContain(
      "Couldn’t open this image in its default app.",
    );
    expect(
      invokeCalls.some((call) => call.command === "record_recent_notification"),
    ).toBe(true);
    expect(
      invokeCalls.some((call) => call.command === "publish_notification"),
    ).toBe(false);
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

  // content-presentation.md D3: fullscreen is an auxiliary webview with no
  // app-config projection of its own, and must fall back to Quick View's
  // enlarge setting (they are one session), never Preview's.
  it("routes fullscreen's fallback enlarge setting through Quick View's, not Preview's", () => {
    useAppStore.setState({ appData: null });
    useWindowPreferencesStore.setState({
      enlargeSmallImagesInPreview: true,
      enlargeSmallImagesInQuickView: false,
    });

    render(
      <PreviewSurface surface="viewer" hash="image-hash" detail={IMAGE_DETAIL} keyboardActive />,
    );

    // Enlarge OFF with known original dimensions caps the image at its real
    // size through an explicit inline style, rather than the enlarge-on
    // behavior of filling the available space.
    const img = screen.getByAltText("family.jpg");
    expect(img.getAttribute("style")).toContain(`max-width: ${IMAGE_DETAIL.width}px`);
  });

  // R5.3 untested contract: in MAIN's own webview (which has `appData.config`
  // directly, unlike the auxiliary windows covered above), the persistent
  // Preview pane reads `enlargeSmallImagesInPreview` and Quick View reads
  // `enlargeSmallImagesInQuickView` -- two independent settings, routed by
  // `surface`, not one shared value.
  it("routes Main's own enlarge setting by surface: Preview reads its own key, Quick View reads its own", () => {
    useAppStore.setState({
      appData: {
        config: {
          videoAutoplay: false,
          audioAutoplay: false,
          soundEnabled: true,
          playbackVolume: 1,
          enlargeSmallImagesInPreview: false,
          enlargeSmallImagesInQuickView: true,
        },
        state: null,
        dataRoot: "/app",
        debugEnabled: false,
        quarantines: [],
      },
    });

    const previewPane = render(
      <PreviewSurface surface="preview-split" hash="image-hash" detail={IMAGE_DETAIL} />,
    );
    const previewImg = previewPane.getByAltText("family.jpg");
    expect(previewImg.getAttribute("style")).toContain(`max-width: ${IMAGE_DETAIL.width}px`);
    previewPane.unmount();

    const quickView = render(
      <PreviewSurface surface="quick" hash="image-hash" detail={IMAGE_DETAIL} keyboardActive />,
    );
    const quickImg = quickView.getByAltText("family.jpg");
    expect(quickImg.getAttribute("style") ?? "").not.toContain("max-width");
  });

  // content-presentation.md D6: a failed video's poster is a PLAIN poster,
  // not an inspectable original — holding it must never raise a bogus
  // "original pixels failed" notice, since the "original" would be the video
  // file itself, and decoding that as an image always fails.
  it("shows a plain, non-inspectable poster after playback fails, with no hold failure notice", () => {
    render(
      <PreviewSurface surface="quick" hash="video-hash" detail={DETAIL} keyboardActive />,
    );

    const video = document.querySelector("video")!;
    fireEvent.error(video);
    const alertsAfterPlaybackFailure = screen.getAllByRole("alert").length;
    const notificationsAfterPlaybackFailure = invokeCalls.filter(
      (call) => call.command === "record_recent_notification",
    ).length;

    const poster = screen.getByAltText(DETAIL.fileName);
    fireEvent.pointerDown(poster, { pointerId: 1 });
    // Holding the poster raises no SECOND (hold-inspection) failure — only
    // the one already-truthful playback-failure notice remains.
    expect(screen.getAllByRole("alert")).toHaveLength(alertsAfterPlaybackFailure);
    expect(
      invokeCalls.filter((call) => call.command === "record_recent_notification"),
    ).toHaveLength(notificationsAfterPlaybackFailure);
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
      await screen.findByText("This audio could not be played in the app."),
    ).toBeTruthy();
    expect(screen.getByText(/first line/)).toBeTruthy();
    expect(await screen.findByText("hello")).toBeTruthy();
  });
});
