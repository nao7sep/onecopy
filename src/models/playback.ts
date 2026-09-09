export type PlaybackMedium = "video" | "audio";
export type PlaybackSurface = "preview-split" | "preview-window" | "quick" | "viewer";

/** Standard MediaError codes do not establish a specific codec or file defect. */
export function playbackFailureMessage(medium: PlaybackMedium, code: number | null): string {
  switch (code) {
    case 1: return `${medium === "video" ? "Video" : "Audio"} playback was interrupted.`;
    case 2: return `This ${medium} could not be read. The file or its connection may be unavailable.`;
    case 3: return `This ${medium} could not be decoded. It may be damaged or use unsupported features.`;
    case 4: return `This ${medium} could not be loaded. Its format may be unsupported, or the file may be unavailable.`;
    default: return `This ${medium} could not be played in the app.`;
  }
}

export interface PlaybackRegistration {
  surface: PlaybackSurface;
  key: string;
  medium: PlaybackMedium;
}

export interface PlaybackSession {
  key: string;
  medium: PlaybackMedium;
  owner: PlaybackSurface | null;
  position: number;
  playing: boolean;
  soundEnabled: boolean;
  volume: number;
}

const SURFACE_PRIORITY: Record<PlaybackSurface, number> = {
  "preview-split": 1,
  "preview-window": 2,
  quick: 3,
  viewer: 4,
};

export function clampPlaybackVolume(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value)
    ? Math.min(1, Math.max(0.01, value))
    : 1;
}

export function choosePlaybackSession(
  registrations: Iterable<PlaybackRegistration>,
  current: PlaybackSession | null,
  policy: {
    videoAutoplay: boolean;
    audioAutoplay: boolean;
    soundEnabled: boolean;
    volume: number;
  },
): PlaybackSession | null {
  const desired = [...registrations].sort(
    (left, right) => SURFACE_PRIORITY[right.surface] - SURFACE_PRIORITY[left.surface],
  )[0];
  if (desired === undefined) return null;
  const sameContent = current?.key === desired.key && current.medium === desired.medium;
  return {
    key: desired.key,
    medium: desired.medium,
    owner: desired.surface,
    position: sameContent ? current.position : 0,
    playing:
      sameContent
        ? current.playing
        : desired.medium === "video"
          ? policy.videoAutoplay
          : policy.audioAutoplay,
    soundEnabled: policy.soundEnabled,
    volume: clampPlaybackVolume(policy.volume),
  };
}
