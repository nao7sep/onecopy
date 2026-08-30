import { create } from "zustand";
import { emit, listen } from "@tauri-apps/api/event";
import type { PlaybackSession } from "../models/playback";
import { log, toErrorFields } from "../repositories";

interface PlaybackClientState {
  session: PlaybackSession | null;
  coordinatorEpoch: number;
}

export const usePlaybackClientStore = create<PlaybackClientState>(() => ({
  session: null,
  coordinatorEpoch: 0,
}));

let installation: Promise<void> | null = null;

/** One listener per webview; individual media bodies only register their
 * availability and observe this local projection. */
export function installPlaybackClient(): Promise<void> {
  if (installation !== null) return installation;
  installation = Promise.all([
    listen<PlaybackSession | null>("playback://state", (event) => {
      usePlaybackClientStore.setState({ session: event.payload });
    }),
    listen("playback://coordinator-ready", () => {
      usePlaybackClientStore.setState((state) => ({
        coordinatorEpoch: state.coordinatorEpoch + 1,
      }));
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
