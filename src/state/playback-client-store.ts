import { create } from "zustand";
import { emit, listen } from "@tauri-apps/api/event";
import type {
  PlaybackRegistration,
  PlaybackSession,
  PlaybackSurface,
} from "../models/playback";
import { log, toErrorFields } from "../repositories";

interface PlaybackClientState {
  session: PlaybackSession | null;
}

export const usePlaybackClientStore = create<PlaybackClientState>(() => ({
  session: null,
}));

let installation: Promise<void> | null = null;
const registrations = new Map<PlaybackSurface, PlaybackRegistration>();

function emitPlayback(event: string, payload: unknown): void {
  void emit(event, payload).catch((error) => {
    log.error("playback event delivery failed", {
      event,
      ...toErrorFields(error),
    });
  });
}

function sameRegistration(
  left: PlaybackRegistration | undefined,
  right: PlaybackRegistration,
): boolean {
  return (
    left?.surface === right.surface &&
    left.key === right.key &&
    left.medium === right.medium
  );
}

function announceRegistrations(): void {
  for (const registration of registrations.values()) {
    emitPlayback("playback://register", registration);
  }
}

/** One listener per webview; individual media bodies only register their
 * availability and observe this local projection. */
export function installPlaybackClient(): Promise<void> {
  if (installation !== null) return installation;
  installation = Promise.all([
    listen<PlaybackSession | null>("playback://state", (event) => {
      usePlaybackClientStore.setState({ session: event.payload });
    }),
    listen("playback://coordinator-ready", () => {
      announceRegistrations();
    }),
  ])
    .then(() => emit("playback://client-ready", {}))
    .then(() => undefined)
    .catch((error) => {
      installation = null;
      log.error("playback state listener failed", toErrorFields(error));
      throw error;
    });
  return installation;
}

/** Registers one live media body without coupling coordinator recovery to a
 * React remount. A coordinator-ready handshake simply re-announces the live
 * registrations, so no stale cleanup can race the replacement registration. */
export function registerPlaybackClient(
  registration: PlaybackRegistration,
): Promise<void> {
  registrations.set(registration.surface, registration);
  return installPlaybackClient().then(() => {
    const current = registrations.get(registration.surface);
    if (sameRegistration(current, registration)) {
      emitPlayback("playback://register", registration);
    }
  });
}

export function unregisterPlaybackClient(
  registration: PlaybackRegistration,
): void {
  if (!sameRegistration(registrations.get(registration.surface), registration))
    return;
  registrations.delete(registration.surface);
  emitPlayback("playback://unregister", registration);
}
