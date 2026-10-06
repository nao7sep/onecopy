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
import { reportStatePatchFailure, useAppStore } from "../state/app-store";
import { usePreviewStore } from "../state/preview-store";
import { useFullscreenViewStore } from "../state/fullscreen-view-store";
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

function policy() {
  return {
    videoAutoplay: booleanConfig("videoAutoplay"),
    audioAutoplay: booleanConfig("audioAutoplay"),
    soundEnabled: booleanConfig("soundEnabled"),
    volume: clampPlaybackVolume(
      useAppStore.getState().appData?.config?.playbackVolume,
    ),
  };
}

function broadcast(): void {
  void emit("playback://state", session).catch((error) => {
    log.error("playback state broadcast failed", toErrorFields(error));
  });
}

function shouldRetainUnownedSession(current: PlaybackSession): boolean {
  const viewer = useFullscreenViewStore.getState();
  if (viewer.currentKey() === current.key) return true;
  const preview = usePreviewStore.getState();
  const previewKey =
    preview.current?.hash ??
    (preview.current?.pathId === null || preview.current?.pathId === undefined
      ? null
      : keyOf(null, preview.current.pathId));
  return preview.follow && previewKey === current.key;
}

/** The preview never becomes a playback owner while the fullscreen view is
 * open: the preview follows the same anchor behind it, and two surfaces
 * playing one item would be heard twice. A fullscreen-view registration
 * briefly missing mid-item-change (the view drops the old item's registration
 * before the preview catches up to the new anchor) must not hand the live
 * session to the preview for that gap — the session being OPEN is what
 * reserves the slot, not merely having a registration this instant. */
function eligibleRegistrations(): PlaybackRegistration[] {
  const all = [...registrations.values()];
  if (useFullscreenViewStore.getState().session === null) return all;
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

// Sound and volume are config settings. The player reports every volume tick
// while the user drags, so writes coalesce on a short timer and run one at a
// time, as app-store does for state; the new values are published
// optimistically so the policy subscription never reads a stale pair back.
const CONFIG_FLUSH_MS = 400;
let pendingConfigPatch: Record<string, unknown> | null = null;
let configFlushTimer: ReturnType<typeof setTimeout> | null = null;
let configWriteTail: Promise<void> = Promise.resolve();

function flushConfigPatch(): Promise<void> {
  if (configFlushTimer !== null) clearTimeout(configFlushTimer);
  configFlushTimer = null;
  const patch = pendingConfigPatch;
  pendingConfigPatch = null;
  if (patch === null) return configWriteTail;
  const write = configWriteTail.then(() =>
    useAppStore.getState().saveConfig(patch, { reportFailure: false }),
  );
  configWriteTail = write.catch(() => undefined);
  return write;
}

/** Writes any coalesced sound/volume change before the app exits. */
export async function flushPlaybackConfigForShutdown(): Promise<void> {
  await flushConfigPatch();
}

function queueConfigPatch(patch: Record<string, unknown>): void {
  useAppStore.setState((s) =>
    s.appData === null
      ? s
      : { appData: { ...s.appData, config: { ...s.appData.config, ...patch } } },
  );
  pendingConfigPatch = { ...(pendingConfigPatch ?? {}), ...patch };
  if (configFlushTimer !== null) clearTimeout(configFlushTimer);
  configFlushTimer = setTimeout(() => {
    void flushConfigPatch().catch(reportStatePatchFailure);
  }, CONFIG_FLUSH_MS);
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
    queueConfigPatch({ soundEnabled, playbackVolume: volume });
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
    // Reserve/release the owner slot the instant the fullscreen view opens or closes,
    // rather than waiting for the next register/unregister to happen to
    // recompute it.
    listeners.retain(useFullscreenViewStore.subscribe((state, previous) => {
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
    pendingConfigPatch = { ...(pendingConfigPatch ?? {}), soundEnabled: enabled };
    await flushConfigPatch();
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
  await useAppStore.getState().saveConfig(
    { [medium === "video" ? "videoAutoplay" : "audioAutoplay"]: enabled },
    { reportFailure: false },
  );
}
