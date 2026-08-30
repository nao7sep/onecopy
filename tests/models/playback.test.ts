import { describe, expect, it } from "vitest";
import { choosePlaybackSession, type PlaybackSession } from "../../src/models/playback";

const policy = {
  videoAutoplay: true,
  audioAutoplay: false,
  soundEnabled: true,
  volume: 0.6,
};

describe("playback ownership", () => {
  it("gives the transient viewer priority over persistent Preview", () => {
    const session = choosePlaybackSession(
      [
        { surface: "preview-split", key: "clip", medium: "video" },
        { surface: "quick", key: "clip", medium: "video" },
      ],
      null,
      policy,
    );
    expect(session?.owner).toBe("quick");
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
      [{ surface: "viewer", key: "clip", medium: "video" }],
      current,
      policy,
    );
    expect(session).toMatchObject({ owner: "viewer", position: 12.5, playing: false });
  });

  it("starts genuinely new audio from the beginning under audio policy", () => {
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
    expect(session).toMatchObject({ key: "memo", position: 0, playing: false });
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
