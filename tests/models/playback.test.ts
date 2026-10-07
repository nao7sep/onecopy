import { describe, expect, it } from "vitest";
import { choosePlaybackSession, playbackFailureMessage, type PlaybackSession } from "../../src/models/playback";
import { t } from "../helpers/i18n";

const policy = {
  autoplay: true,
  soundEnabled: true,
  volume: 0.6,
};

describe("playback failure messages", () => {
  it.each([
    [1, "interrupted"], [2, "could not be read"], [3, "could not be decoded"],
    [4, "could not be loaded"], [null, "could not be played"], [99, "could not be played"],
  ] as const)("reports code %s without inventing an exact codec cause", (code, words) => {
    for (const medium of ["video", "audio"] as const) {
      const message = t(playbackFailureMessage(medium, code));
      expect(message).toContain(words);
      expect(message.toLowerCase()).toContain(medium);
      expect(message).not.toContain("This codec");
    }
  });
});

describe("playback ownership", () => {
  it("gives the fullscreen view priority over the preview", () => {
    const session = choosePlaybackSession(
      [
        { surface: "preview-split", key: "clip", medium: "video" },
        { surface: "fullscreen-view", key: "clip", medium: "video" },
      ],
      null,
      policy,
    );
    expect(session?.owner).toBe("fullscreen-view");
    expect(session?.playing).toBe(true);
  });

  it("preserves position and playing state while the same item changes surfaces", () => {
    const current: PlaybackSession = {
      key: "clip",
      medium: "video",
      owner: "preview-window",
      position: 12.5,
      playing: false,
      soundEnabled: true,
      volume: 0.8,
    };
    const session = choosePlaybackSession(
      [{ surface: "fullscreen-view", key: "clip", medium: "video" }],
      current,
      policy,
    );
    expect(session).toMatchObject({ owner: "fullscreen-view", position: 12.5, playing: false });
  });

  it("starts genuinely new audio from the beginning under the shared autoplay policy", () => {
    const session = choosePlaybackSession(
      [{ surface: "preview-split", key: "memo", medium: "audio" }],
      {
        key: "clip",
        medium: "video",
        owner: "preview-split",
        position: 40,
        playing: true,
        soundEnabled: true,
        volume: 1,
      },
      policy,
    );
    expect(session).toMatchObject({ key: "memo", position: 0, playing: true });
  });

  it("does not revive an old position after another logical item took over", () => {
    const session = choosePlaybackSession(
      [{ surface: "preview-split", key: "clip", medium: "video" }],
      {
        key: "other-clip",
        medium: "video",
        owner: "preview-split",
        position: 40,
        playing: false,
        soundEnabled: true,
        volume: 1,
      },
      policy,
    );

    expect(session).toMatchObject({ key: "clip", position: 0, playing: true });
  });

  it("applies Sound and volume policy without changing a live item state", () => {
    const session = choosePlaybackSession(
      [{ surface: "preview-split", key: "clip", medium: "video" }],
      {
        key: "clip",
        medium: "video",
        owner: "preview-split",
        position: 18,
        playing: true,
        soundEnabled: true,
        volume: 0.8,
      },
      { ...policy, soundEnabled: false, volume: 0.4 },
    );

    expect(session).toMatchObject({
      position: 18,
      playing: true,
      soundEnabled: false,
      volume: 0.4,
    });
  });
});
