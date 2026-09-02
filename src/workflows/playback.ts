import { emit, listen } from "@tauri-apps/api/event";
import {
  choosePlaybackSession,
  clampPlaybackVolume,
  type PlaybackMedium,
  type PlaybackRegistration,
  type PlaybackSession,
  type PlaybackSurface,
} from "../models/playback";
import { log, toErrorFields } from "../repositories";
import { retainStatePatch, useAppStore } from "../state/app-store";
import { usePreviewStore } from "../state/preview-store";
import { useQuickViewStore } from "../state/quick-view-store";

interface PlaybackObservation {
  surface: PlaybackSurface;
  key: string;
  position: number;
  playing: boolean;
  volume: number;
  muted: boolean;
}

interface PlaybackTarget {
  key: string;
  position?: number;
  play?: boolean;
}

const registrations = new Map<PlaybackSurface, PlaybackRegistration>();
let session: PlaybackSession | null = null;
let installed = false;
let pendingState: Record<string, unknown> | null = null;
let stateTimer: ReturnType<typeof setTimeout> | null = null;
let pendingSeek: PlaybackTarget | null = null;

function booleanConfig(key: string, fallback = true): boolean {
  const value = useAppStore.getState().appData?.config?.[key];
  return typeof value === "boolean" ? value : fallback;
}

function booleanState(key: string, fallback = true): boolean {
  const value = useAppStore.getState().appData?.state?.[key];
  return typeof value === "boolean" ? value : fallback;
}

function policy() {
  return {
    videoAutoplay: booleanConfig("videoAutoplay"),
    audioAutoplay: booleanConfig("audioAutoplay"),
    soundEnabled: booleanState("soundEnabled"),
    volume: clampPlaybackVolume(
      useAppStore.getState().appData?.state?.playbackVolume,
    ),
  };
}

function broadcast(): void {
  void emit("playback://state", session).catch((error) => {
    log.error("playback state broadcast failed", toErrorFields(error));
  });
}

function shouldRetainUnownedSession(current: PlaybackSession): boolean {
  const viewer = useQuickViewStore.getState();
  if (viewer.currentKey() === current.key) return true;
  const preview = usePreviewStore.getState();
  const previewKey =
    preview.current?.hash ??
    (preview.current?.pathId === null || preview.current?.pathId === undefined
      ? null
      : `path-${preview.current.pathId}`);
  return preview.follow && previewKey === current.key;
}

function recompute(): void {
  const next = choosePlaybackSession(registrations.values(), session, policy());
  if (
    next === null &&
    session !== null &&
    shouldRetainUnownedSession(session)
  ) {
    session = { ...session, owner: null };
  } else {
    session = next;
  }
  if (
    session !== null &&
    pendingSeek?.key === session.key &&
    Number.isFinite(pendingSeek.position)
  ) {
    session = {
      ...session,
      position: Math.max(0, pendingSeek.position ?? 0),
      playing: pendingSeek.play ?? true,
    };
    pendingSeek = null;
  }
  broadcast();
}

function register(registration: PlaybackRegistration): void {
  registrations.set(registration.surface, registration);
  recompute();
}

function unregister(registration: PlaybackRegistration): void {
  const current = registrations.get(registration.surface);
  if (current?.key !== registration.key) return;
  registrations.delete(registration.surface);
  recompute();
}

function queueStatePatch(patch: Record<string, unknown>): void {
  pendingState = { ...(pendingState ?? {}), ...patch };
  if (stateTimer !== null) clearTimeout(stateTimer);
  stateTimer = setTimeout(() => {
    const next = pendingState;
    pendingState = null;
    stateTimer = null;
    if (next === null) return;
    retainStatePatch(next);
  }, 250);
}

function observe(observation: PlaybackObservation): void {
  if (
    session === null ||
    session.owner !== observation.surface ||
    session.key !== observation.key
  ) {
    return;
  }
  const position = Number.isFinite(observation.position)
    ? Math.max(0, observation.position)
    : session.position;
  let soundEnabled = session.soundEnabled;
  let volume = session.volume;
  if (observation.muted || observation.volume <= 0) {
    soundEnabled = false;
  } else {
    soundEnabled = true;
    volume = clampPlaybackVolume(observation.volume);
  }
  const soundChanged = soundEnabled !== session.soundEnabled;
  const volumeChanged = volume !== session.volume;
  const playbackChanged =
    position !== session.position || observation.playing !== session.playing;
  session = {
    ...session,
    position,
    playing: observation.playing,
    soundEnabled,
    volume,
  };
  if (soundChanged || volumeChanged || playbackChanged) {
    broadcast();
  }
  if (soundChanged || volumeChanged) {
    queueStatePatch({ soundEnabled, playbackVolume: volume });
  }
}

function toggle(target: PlaybackTarget): void {
  if (session === null || session.key !== target.key || session.owner === null)
    return;
  session = { ...session, playing: !session.playing };
  broadcast();
}

function pause(target: PlaybackTarget): void {
  if (session === null || session.key !== target.key || !session.playing)
    return;
  session = { ...session, playing: false };
  broadcast();
}

function seek(target: PlaybackTarget): void {
  if (
    session === null ||
    session.key !== target.key ||
    !Number.isFinite(target.position)
  ) {
    if (Number.isFinite(target.position)) pendingSeek = target;
    return;
  }
  session = {
    ...session,
    position: Math.max(0, target.position ?? 0),
    playing: target.play ?? true,
  };
  broadcast();
}

/** Main-webview coordinator for the one live playback session. */
export async function installPlaybackWorkflow(): Promise<void> {
  if (installed) return;
  installed = true;
  await Promise.all([
    listen<PlaybackRegistration>("playback://register", (event) =>
      register(event.payload),
    ),
    listen<PlaybackRegistration>("playback://unregister", (event) =>
      unregister(event.payload),
    ),
    listen<PlaybackObservation>("playback://observe", (event) =>
      observe(event.payload),
    ),
    listen<PlaybackTarget>("playback://toggle", (event) =>
      toggle(event.payload),
    ),
    listen<PlaybackTarget>("playback://pause", (event) => pause(event.payload)),
    listen<PlaybackTarget>("playback://seek", (event) => seek(event.payload)),
    listen("playback://client-ready", () => {
      void emit("playback://coordinator-ready", {}).catch((error) => {
        log.error("playback handshake failed", toErrorFields(error));
      });
    }),
  ]);
  useAppStore.subscribe((state, previous) => {
    if (
      (state.appData?.config === previous.appData?.config &&
        state.appData?.state === previous.appData?.state) ||
      session === null
    )
      return;
    const next = policy();
    session = {
      ...session,
      soundEnabled: next.soundEnabled,
      volume: next.volume,
    };
    broadcast();
  });
  await emit("playback://coordinator-ready", {});
}

export function toggleMainPlayback(key: string): boolean {
  if (session === null || session.key !== key || session.owner === null)
    return false;
  toggle({ key });
  return true;
}

export function seekMainPlayback(key: string, position: number): void {
  seek({ key, position });
}

/** Cross-webview explicit seek used by transcript timestamps. */
export function requestPlaybackSeek(key: string, position: number): void {
  void emit("playback://seek", { key, position, play: true }).catch((error) => {
    log.error("playback seek delivery failed", toErrorFields(error));
  });
}

export async function setSoundEnabled(enabled: boolean): Promise<void> {
  if (session !== null) {
    session = { ...session, soundEnabled: enabled };
    broadcast();
  }
  await useAppStore.getState().patchState(
    { soundEnabled: enabled },
    { immediate: true },
  );
}

export async function setMediumAutoplay(
  medium: PlaybackMedium,
  enabled: boolean,
): Promise<void> {
  await useAppStore.getState().patchConfig({
    [medium === "video" ? "videoAutoplay" : "audioAutoplay"]: enabled,
  });
}
