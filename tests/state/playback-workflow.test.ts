import { beforeEach, describe, expect, it } from "vitest";
import type { PlaybackSession } from "../../src/models/playback";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import type { ActiveViewerSession } from "../../src/models/viewerSession";
import { useAppStore } from "../../src/state/app-store";
import { usePreviewStore } from "../../src/state/preview-store";
import { useFullscreenViewStore } from "../../src/state/fullscreen-view-store";
import { handleFullscreenViewKey } from "../../src/workflows/fullscreen-view";
import { installPlaybackWorkflow, setSoundEnabled, setPlaybackVolume, setAutoplay, flushPlaybackConfigForShutdown } from "../../src/workflows/playback";
import { emitCalls, fireEvent, mockCommands, resetTauriMocks } from "../mocks/tauri";

function latestState(): PlaybackSession | null {
  const call = [...emitCalls]
    .reverse()
    .find((entry) => entry.event === "playback://state");
  return (call?.payload as PlaybackSession | null | undefined) ?? null;
}

beforeEach(async () => {
  resetTauriMocks({ keepListeners: true });
  useAppStore.setState({
    appData: {
      config: {
        autoplay: true,
        soundEnabled: true,
        playbackVolume: 0.7,
      },
      state: {},
      dataRoot: "/app",
      debugEnabled: false,
      quarantines: [],
    },
  });
  usePreviewStore.setState({ follow: true });
  useFullscreenViewStore.setState({ session: null });
  await installPlaybackWorkflow();
});

describe("sound setting", () => {
  it("retains the selected Sound state after its save fails and retries it", async () => {
    fireEvent("playback://register", { surface: "preview-split", key: "clip", medium: "video" });
    mockCommands({
      save_config: () => Promise.reject(new TypeError("EACCES writing config.json")),
    });

    await expect(setSoundEnabled(false)).rejects.toBeInstanceOf(TypeError);

    expect(latestState()).toMatchObject({ soundEnabled: false });
    expect(useAppStore.getState().appData?.config).toMatchObject({ soundEnabled: false });
    mockCommands({ save_config: ({ changes }) => changes });
    await flushPlaybackConfigForShutdown();
    expect(latestState()).toMatchObject({ soundEnabled: false });
  });

  it("keeps a saved Sound change", async () => {
    fireEvent("playback://register", { surface: "preview-split", key: "clip", medium: "video" });
    mockCommands({ save_config: ({ changes }) => changes });

    await setSoundEnabled(false);

    expect(latestState()).toMatchObject({ soundEnabled: false });
    expect(useAppStore.getState().appData?.config).toMatchObject({ soundEnabled: false });
  });
  it("keeps a later Sound choice when an earlier save fails", async () => {
    let failFirst!: (error: Error) => void;
    let calls = 0;
    mockCommands({ save_config: ({ changes }) => {
      if (++calls === 1) return new Promise((_, reject) => { failFirst = reject; });
      return { ...useAppStore.getState().appData!.config, ...(changes as object) };
    } });
    const first = setSoundEnabled(false);
    const failed = expect(first).rejects.toThrow("disk full");
    await Promise.resolve();
    const later = setSoundEnabled(true);
    failFirst(new Error("disk full"));
    await failed;
    await later;
    expect(useAppStore.getState().appData?.config.soundEnabled).toBe(true);
    await flushPlaybackConfigForShutdown();
    expect(calls).toBe(2);
  });
});

describe("unified playback policy", () => {
  it("applies the same autoplay choice when navigating to either audio or video", async () => {
    mockCommands({ save_config: ({ changes }) => ({ ...useAppStore.getState().appData!.config, ...(changes as object) }) });
    for (const autoplay of [false, true]) {
      await setAutoplay(autoplay);
      for (const medium of ["audio", "video"]) {
        fireEvent("playback://register", { surface: "fullscreen-view", key: `${medium}-${autoplay}`, medium });
        expect(latestState()?.playing).toBe(autoplay);
        fireEvent("playback://unregister", { surface: "fullscreen-view", key: `${medium}-${autoplay}`, medium });
      }
    }
  });
  it("does not roll a newer volume choice back when an earlier save finishes", async () => {
    let finishFirst!: (value: unknown) => void;
    let calls = 0;
    mockCommands({ save_config: ({ changes }) => {
      const saved = { ...useAppStore.getState().appData!.config, ...(changes as object) };
      if (++calls === 1) return new Promise((resolve) => { finishFirst = () => resolve(saved); });
      return saved;
    } });
    setPlaybackVolume(0.3);
    const first = flushPlaybackConfigForShutdown();
    await Promise.resolve();
    setPlaybackVolume(0.8);
    finishFirst(null);
    await first;
    expect(useAppStore.getState().appData?.config.playbackVolume).toBe(0.8);
    await flushPlaybackConfigForShutdown();
    expect(useAppStore.getState().appData?.config.playbackVolume).toBe(0.8);
  });

  it("retries a failed authored patch with later edits instead of losing or replaying them", async () => {
    let failFirst!: (error: Error) => void;
    const writes: Array<Record<string, unknown>> = [];
    mockCommands({ save_config: ({ changes }) => {
      writes.push(changes as Record<string, unknown>);
      if (writes.length === 1) return new Promise((_, reject) => { failFirst = reject; });
      return { ...useAppStore.getState().appData!.config, ...(changes as object) };
    } });
    setPlaybackVolume(0.3);
    const first = flushPlaybackConfigForShutdown();
    const failed = expect(first).rejects.toThrow("disk full");
    await Promise.resolve();
    setPlaybackVolume(0.8);
    const retry = flushPlaybackConfigForShutdown();
    failFirst(new Error("disk full"));
    await failed;
    await retry;
    expect(writes).toEqual([{ soundEnabled: true, playbackVolume: 0.3 }, { soundEnabled: true, playbackVolume: 0.8 }]);
    await flushPlaybackConfigForShutdown();
    expect(writes).toHaveLength(2);
    expect(useAppStore.getState().appData?.config.playbackVolume).toBe(0.8);
  });

  it("updates the active player immediately and keeps its position through volume and mute", async () => {
    mockCommands({ save_config: ({ changes }) => ({ ...useAppStore.getState().appData!.config, ...(changes as object) }) });
    fireEvent("playback://register", { surface: "fullscreen-view", key: "volume-clip", medium: "video" });
    fireEvent("playback://observe", { surface: "fullscreen-view", key: "volume-clip", position: 12, playing: true, volume: 0.7, muted: false });
    setPlaybackVolume(0.2);
    expect(latestState()).toMatchObject({ position: 12, playing: true, volume: 0.2, soundEnabled: true });
    setPlaybackVolume(0);
    expect(latestState()).toMatchObject({ position: 12, volume: 0.2, soundEnabled: false });
    await setSoundEnabled(true);
    expect(latestState()).toMatchObject({ volume: 0.2, soundEnabled: true });
    await flushPlaybackConfigForShutdown();
    fireEvent("playback://unregister", { surface: "fullscreen-view", key: "volume-clip", medium: "video" });
  });
});

describe("playback workflow", () => {
  it("hands one live session to the highest surface and back without losing state", () => {
    fireEvent("playback://register", {
      surface: "preview-split",
      key: "clip",
      medium: "video",
    });
    expect(latestState()).toMatchObject({
      owner: "preview-split",
      key: "clip",
      playing: true,
      volume: 0.7,
    });

    fireEvent("playback://register", {
      surface: "fullscreen-view",
      key: "clip",
      medium: "video",
    });
    fireEvent("playback://observe", {
      surface: "fullscreen-view",
      key: "clip",
      position: 12.5,
      playing: false,
      volume: 0.7,
      muted: false,
    });
    fireEvent("playback://unregister", {
      surface: "fullscreen-view",
      key: "clip",
      medium: "video",
    });

    expect(latestState()).toMatchObject({
      owner: "preview-split",
      key: "clip",
      position: 12.5,
      playing: false,
    });
    fireEvent("playback://unregister", {
      surface: "preview-split",
      key: "clip",
      medium: "video",
    });
  });

  it("pauses the live session before external delegation", () => {
    fireEvent("playback://register", {
      surface: "preview-split",
      key: "external-clip",
      medium: "video",
    });
    fireEvent("playback://pause", { key: "external-clip" });

    expect(latestState()).toMatchObject({
      owner: "preview-split",
      key: "external-clip",
      playing: false,
    });
    fireEvent("playback://unregister", {
      surface: "preview-split",
      key: "external-clip",
      medium: "video",
    });
  });

  it("retains an ownerless session only for a real same-item surface handoff", () => {
    usePreviewStore.setState({
      follow: true,
      current: { hash: "handoff", pathId: null, detail: null },
    });
    fireEvent("playback://register", {
      surface: "preview-split",
      key: "handoff",
      medium: "video",
    });
    fireEvent("playback://unregister", {
      surface: "preview-split",
      key: "handoff",
      medium: "video",
    });
    expect(latestState()).toMatchObject({ key: "handoff", owner: null });

    usePreviewStore.setState({ current: null });
    fireEvent("playback://register", {
      surface: "preview-split",
      key: "leaving",
      medium: "video",
    });
    fireEvent("playback://unregister", {
      surface: "preview-split",
      key: "leaving",
      medium: "video",
    });
    expect(latestState()).toBeNull();
  });

  // One surface owns playback at a time: while the fullscreen view is open
  // for the same content, Preview follows it instead of becoming a second
  // owner.
  it("never hands the live session to Preview while the fullscreen view is open for the same content", () => {
    fireEvent("playback://register", {
      surface: "preview-split",
      key: "clip",
      medium: "video",
    });
    expect(latestState()).toMatchObject({ owner: "preview-split", key: "clip" });

    // The fullscreen view opens for the SAME item and briefly has no
    // registration of its own (the item change drops the old registration before the new
    // one lands) — Preview must not reclaim ownership during that gap.
    useFullscreenViewStore.setState({
      session: { member: { hash: "clip", pathId: null } } as never,
    });
    expect(latestState()).toMatchObject({ key: "clip", owner: null });

    fireEvent("playback://register", {
      surface: "fullscreen-view",
      key: "clip",
      medium: "video",
    });
    expect(latestState()).toMatchObject({ owner: "fullscreen-view", key: "clip" });

    fireEvent("playback://unregister", {
      surface: "fullscreen-view",
      key: "clip",
      medium: "video",
    });
    // The fullscreen view is still open (now showing something else), so
    // Preview remains ineligible even with no registration of the view's.
    expect(latestState()).toMatchObject({ key: "clip", owner: null });

    useFullscreenViewStore.setState({ session: null });
    expect(latestState()).toMatchObject({ owner: "preview-split", key: "clip" });

    fireEvent("playback://unregister", {
      surface: "preview-split",
      key: "clip",
      medium: "video",
    });
  });

  // content-presentation.md D2: a timestamp seek queued for a key with no
  // live or registering player must not fire on some unrelated later visit.
  describe("seeking a key with no live player", () => {
    it("never survives past the next unrelated recompute to fire on a later, unrelated visit", () => {
      fireEvent("playback://seek", { key: "later", position: 42, play: true });
      // An unrelated registration recomputes the session once — the queued
      // seek is single-shot and is dropped here, not held for "later" only.
      fireEvent("playback://register", {
        surface: "preview-split",
        key: "unrelated",
        medium: "video",
      });
      fireEvent("playback://unregister", {
        surface: "preview-split",
        key: "unrelated",
        medium: "video",
      });
      // Minutes afterward, the same key finally gets a real player: it must
      // start at 0, not jump to the stale click from before.
      fireEvent("playback://register", {
        surface: "preview-split",
        key: "later",
        medium: "video",
      });
      expect(latestState()).toMatchObject({ key: "later", position: 0 });
      fireEvent("playback://unregister", {
        surface: "preview-split",
        key: "later",
        medium: "video",
      });
    });

    it("applies a queued seek only to the very next registration, then drops it", () => {
      // No player is registered for this key YET (it is opening) — the seek
      // queues for whichever registration arrives next.
      fireEvent("playback://seek", { key: "opening", position: 30, play: true });
      fireEvent("playback://register", {
        surface: "preview-split",
        key: "opening",
        medium: "video",
      });
      expect(latestState()).toMatchObject({ key: "opening", position: 30, playing: true });

      fireEvent("playback://unregister", {
        surface: "preview-split",
        key: "opening",
        medium: "video",
      });
      // The seek was single-shot: reusing the same key on a LATER,
      // independent registration never replays it.
      fireEvent("playback://register", {
        surface: "preview-split",
        key: "opening",
        medium: "video",
      });
      expect(latestState()).toMatchObject({ key: "opening", position: 0 });
      fireEvent("playback://unregister", {
        surface: "preview-split",
        key: "opening",
        medium: "video",
      });
    });
  });
});

// Enter in the fullscreen view toggles the centrally owned playback session
// for the current video item, as clicking the picture does
// (preview-surface.test.tsx holds the click half).
describe("fullscreen view Enter toggle", () => {
  it("toggles the owned session's playing state on Enter for a video item", async () => {
    fireEvent("playback://register", {
      surface: "fullscreen-view",
      key: "clip",
      medium: "video",
    });
    expect(latestState()).toMatchObject({ key: "clip", owner: "fullscreen-view", playing: true });

    const item: SectionItem = {
      hash: "clip",
      pathId: 1,
      fileName: "clip.mov",
      resolvedUtcMs: 1,
      copyCount: 1,
      width: 100,
      height: 100,
      hasThumb: true,
      similarGroupId: null,
      sharpness: null,
      faceScore: null,
      byteSize: 1000,
      hasCompanions: false,
      durationMs: 5000,
      dirPaths: [],
      derivedWork: EMPTY_ITEM_WORK,
    };
    const session: ActiveViewerSession = {
      token: "fullscreen-view-token",
      member: { hash: "clip", pathId: 1 },
      item,
      detail: {
        fileName: "clip.mov", kind: "video", byteSize: 1000, width: 100, height: 100,
        durationMs: 5000, dateState: "dated", resolvedUtcMs: 1, resolvedSource: "metadata",
        dateOnly: false, copyPaths: ["/videos/clip.mov"], companionPaths: [], stripFrames: null,
      },
      index: 0,
      length: 1,
      sectionIndex: 0,
      scope: "section",
      main: {
        projection: { section: null, sort: { order: "time", desc: false }, revision: 0 },
        selectedKeys: ["clip"],
        frozenPositionsValid: true,
      },
    };
    useFullscreenViewStore.setState({ session, pendingDelete: null, failure: null });

    await handleFullscreenViewKey({ key: "Enter" });

    expect(latestState()).toMatchObject({ key: "clip", playing: false });

    await handleFullscreenViewKey({ key: "Enter" });

    expect(latestState()).toMatchObject({ key: "clip", playing: true });
  });
});
