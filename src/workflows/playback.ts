import { emit } from "@tauri-apps/api/event";
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
import { createEventInstaller } from "../utils/eventInstallation";
import { keyOf } from "../models/items";

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
      : keyOf(null, preview.current.pathId));
  return preview.follow && previewKey === current.key;
}

/** Persistent Preview never becomes another playback owner while the
 * transient viewer is open for the same content (viewing-sessions.md: "Main
 * keeps ... persistent Preview follows it without becoming another playback
 * owner"; content-presentation.md: "Only one OneCopy surface owns playback").
 * A viewer registration briefly missing mid-item-change (Quick View drops the
 * old item's registration before Preview catches up to the new anchor) must
 * not hand the live session to Preview for that gap — the viewer session
 * being OPEN is what reserves the slot, not merely having a registration this
 * instant. */
function eligibleRegistrations(): PlaybackRegistration[] {
  const all = [...registrations.values()];
  if (useQuickViewStore.getState().session === null) return all;
  return all.filter(
    (registration) => registration.surface !== "preview-split" && registration.surface !== "preview-window",
  );
}

function recompute(): void {
  const next = choosePlaybackSession(eligibleRegistrations(), session, policy());
  if (
    next === null &&
    session !== null &&
    shouldRetainUnownedSession(session)
  ) {
    session = { ...session, owner: null };
  } else {
    session = next;
  }
  if (pendingSeek !== null) {
    if (
      session !== null &&
      pendingSeek.key === session.key &&
      Number.isFinite(pendingSeek.position)
    ) {
      session = {
        ...session,
        position: Math.max(0, pendingSeek.position ?? 0),
        playing: pendingSeek.play ?? true,
      };
    }
    // Single-shot regardless of match: a seek queued for a player that has
    // not registered YET is meant for the very next recompute, not for
    // whatever unrelated item eventually reuses this key later
    // (content-presentation.md D2 — a timestamp with no live session must
    // never leave a seek that fires minutes afterward on a different visit).
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
  retainStatePatch(patch);
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
    // Queued for the IMMEDIATELY NEXT recompute only (see `recompute`'s
    // single-shot consumption) — worth it for a player that is opening this
    // instant and about to register, never for "whenever this key is next
    // reused", which is what let a click minutes ago silently reappear on a
    // later, unrelated visit (content-presentation.md D2).
    pendingSeek = Number.isFinite(target.position) ? target : null;
    return;
  }
  pendingSeek = null;
  session = {
    ...session,
    position: Math.max(0, target.position ?? 0),
    playing: target.play ?? true,
  };
  broadcast();
}

const install = createEventInstaller(
  async (listeners) => {
    await listeners.listen<PlaybackRegistration>("playback://register", (event) =>
      register(event.payload),
    );
    await listeners.listen<PlaybackRegistration>("playback://unregister", (event) =>
      unregister(event.payload),
    );
    await listeners.listen<PlaybackObservation>("playback://observe", (event) =>
      observe(event.payload),
    );
    await listeners.listen<PlaybackTarget>("playback://toggle", (event) =>
      toggle(event.payload),
    );
    await listeners.listen<PlaybackTarget>("playback://pause", (event) => pause(event.payload));
    await listeners.listen<PlaybackTarget>("playback://seek", (event) => seek(event.payload));
    await listeners.listen("playback://client-ready", () => {
      void emit("playback://coordinator-ready", {}).catch((error) => {
        log.error("playback handshake failed", toErrorFields(error));
      });
    });
    // Reserve/release the owner slot the instant the viewer opens or closes,
    // rather than waiting for the next register/unregister to happen to
    // recompute it.
    listeners.retain(useQuickViewStore.subscribe((state, previous) => {
      if ((state.session === null) !== (previous.session === null)) recompute();
    }));
    listeners.retain(useAppStore.subscribe((state, previous) => {
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
    }));
    await emit("playback://coordinator-ready", {});
  },
  (error) => log.error("playback coordinator wiring failed", toErrorFields(error)),
);

/** Main-webview coordinator for the one live playback session. */
export function installPlaybackWorkflow(): Promise<void> {
  return install();
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
  const previous = session?.soundEnabled;
  if (session !== null) {
    session = { ...session, soundEnabled: enabled };
    broadcast();
  }
  try {
    // The caller shows the failure, so the core stays quiet: one failed write
    // is one record.
    await useAppStore.getState().patchState(
      { soundEnabled: enabled },
      { immediate: true, reportFailure: false },
    );
  } catch (error) {
    // Players and the status bar must not keep claiming a setting that was
    // never saved.
    if (session !== null && previous !== undefined) {
      session = { ...session, soundEnabled: previous };
      broadcast();
    }
    throw error;
  }
}

export async function setMediumAutoplay(
  medium: PlaybackMedium,
  enabled: boolean,
): Promise<void> {
  await useAppStore.getState().patchConfig(
    { [medium === "video" ? "videoAutoplay" : "audioAutoplay"]: enabled },
    { reportFailure: false },
  );
}
