import { beforeEach, describe, expect, it } from "vitest";
import type { PlaybackSession } from "../../src/models/playback";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import type { ActiveViewerSession } from "../../src/models/viewerSession";
import { useAppStore } from "../../src/state/app-store";
import { usePreviewStore } from "../../src/state/preview-store";
import { useQuickViewStore } from "../../src/state/quick-view-store";
import { handleViewerKey } from "../../src/workflows/quick-view";
import { installPlaybackWorkflow, setSoundEnabled } from "../../src/workflows/playback";
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
        videoAutoplay: true,
        audioAutoplay: false,
      },
      state: {
        soundEnabled: true,
        playbackVolume: 0.7,
      },
      dataRoot: "/app",
      debugEnabled: false,
      quarantines: [],
    },
  });
  usePreviewStore.setState({ follow: true });
  useQuickViewStore.setState({ session: null });
  await installPlaybackWorkflow();
});

describe("sound setting", () => {
  it("takes Sound back from the players when its write never reached disk", async () => {
    fireEvent("playback://register", { surface: "preview-split", key: "clip", medium: "video" });
    mockCommands({
      patch_state: () => Promise.reject(new TypeError("EACCES writing state.json")),
    });

    await expect(setSoundEnabled(false)).rejects.toBeInstanceOf(TypeError);

    expect(latestState()).toMatchObject({ soundEnabled: true });
    expect(useAppStore.getState().appData?.state).toMatchObject({ soundEnabled: true });
  });

  it("keeps a saved Sound change", async () => {
    fireEvent("playback://register", { surface: "preview-split", key: "clip", medium: "video" });
    mockCommands({ patch_state: ({ patch }) => patch });

    await setSoundEnabled(false);

    expect(latestState()).toMatchObject({ soundEnabled: false });
    expect(useAppStore.getState().appData?.state).toMatchObject({ soundEnabled: false });
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
      surface: "quick",
      key: "clip",
      medium: "video",
    });
    fireEvent("playback://observe", {
      surface: "quick",
      key: "clip",
      position: 12.5,
      playing: false,
      volume: 0.7,
      muted: false,
    });
    fireEvent("playback://unregister", {
      surface: "quick",
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

  // content-presentation.md D1: "Only one OneCopy surface owns playback at a
  // time" and Preview "follows it without becoming another playback owner"
  // while the transient viewer is open for the same content.
  it("never hands the live session to Preview while the viewer is open for the same content", () => {
    fireEvent("playback://register", {
      surface: "preview-split",
      key: "clip",
      medium: "video",
    });
    expect(latestState()).toMatchObject({ owner: "preview-split", key: "clip" });

    // Quick View opens for the SAME item and briefly has no registration of
    // its own (the item change drops the old registration before the new
    // one lands) — Preview must not reclaim ownership during that gap.
    useQuickViewStore.setState({
      session: { presentation: "quick", member: { hash: "clip", pathId: null } } as never,
    });
    expect(latestState()).toMatchObject({ key: "clip", owner: null });

    fireEvent("playback://register", {
      surface: "quick",
      key: "clip",
      medium: "video",
    });
    expect(latestState()).toMatchObject({ owner: "quick", key: "clip" });

    fireEvent("playback://unregister", {
      surface: "quick",
      key: "clip",
      medium: "video",
    });
    // The viewer is still open (now showing something else) — Preview
    // remains ineligible even with no current viewer registration.
    expect(latestState()).toMatchObject({ key: "clip", owner: null });

    useQuickViewStore.setState({ session: null });
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

// content-presentation.md: "Picture click and Enter toggle" (R5.3 untested
// contract) -- Enter in the viewer toggles the CENTRALLY OWNED playback
// session for the current video item, the same way clicking the picture
// itself does (see preview-surface.test.tsx for the click half).
describe("viewer Enter toggle", () => {
  it("toggles the owned session's playing state on Enter for a video item", async () => {
    fireEvent("playback://register", {
      surface: "quick",
      key: "clip",
      medium: "video",
    });
    expect(latestState()).toMatchObject({ key: "clip", owner: "quick", playing: true });

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
      token: "viewer-token",
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
      presentation: "quick",
      main: {
        projection: { section: null, sort: { order: "time", desc: false }, revision: 0 },
        selectedKeys: ["clip"],
        frozenPositionsValid: true,
      },
    };
    useQuickViewStore.setState({ session, pendingDelete: null, failure: null });

    await handleViewerKey({ key: "Enter" });

    expect(latestState()).toMatchObject({ key: "clip", playing: false });

    await handleViewerKey({ key: "Enter" });

    expect(latestState()).toMatchObject({ key: "clip", playing: true });
  });
});
