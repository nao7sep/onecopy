import { create } from "zustand";
import { emit } from "@tauri-apps/api/event";
import type {
  PlaybackRegistration,
  PlaybackSession,
  PlaybackSurface,
} from "../models/playback";
import { log, toErrorFields } from "../repositories";
import { createEventInstaller } from "../utils/eventInstallation";

interface PlaybackClientState {
  session: PlaybackSession | null;
}

export const usePlaybackClientStore = create<PlaybackClientState>(() => ({
  session: null,
}));

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

const install = createEventInstaller(
  async (listeners) => {
    await listeners.listen<PlaybackSession | null>("playback://state", (event) => {
      usePlaybackClientStore.setState({ session: event.payload });
    });
    await listeners.listen("playback://coordinator-ready", () => {
      announceRegistrations();
    });
    await emit("playback://client-ready", {});
  },
  (error) => log.error("playback state listener failed", toErrorFields(error)),
  { propagateFailure: true },
);

/** One listener per webview; individual media bodies only register their
 * availability and observe this local projection. */
export function installPlaybackClient(): Promise<void> {
  return install();
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
