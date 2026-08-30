import { useCallback, useEffect } from "react";
import { emit } from "@tauri-apps/api/event";
import type {
  PlaybackMedium,
  PlaybackRegistration,
  PlaybackSurface,
} from "../models/playback";
import { mediaUseActive, useOwnedMedia } from "../media-use";
import { log, toErrorFields } from "../repositories";
import {
  installPlaybackClient,
  usePlaybackClientStore,
} from "../state/playback-client-store";

function emitPlayback(event: string, payload: unknown): void {
  void emit(event, payload).catch((error) => {
    log.error("playback event delivery failed", {
      event,
      ...toErrorFields(error),
    });
  });
}

export function usePlaybackMedia<T extends HTMLMediaElement>(
  surface: PlaybackSurface,
  key: string,
  medium: PlaybackMedium,
  enabled = true,
) {
  const [elementRef, setElementRef] = useOwnedMedia<T>();
  const session = usePlaybackClientStore((state) => state.session);
  const coordinatorEpoch = usePlaybackClientStore(
    (state) => state.coordinatorEpoch,
  );
  const registration: PlaybackRegistration = { surface, key, medium };

  useEffect(() => {
    if (!enabled) return;
    let registered = false;
    let disposed = false;
    void installPlaybackClient()
      .then(() => {
        if (disposed) return;
        registered = true;
        emitPlayback("playback://register", registration);
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      if (registered) emitPlayback("playback://unregister", registration);
    };
  }, [coordinatorEpoch, enabled, key, medium, surface]);

  useEffect(() => {
    const element = elementRef.current;
    if (element === null) return;
    const owns = enabled && session?.owner === surface && session.key === key;
    if (!owns) {
      element.pause();
      return;
    }
    if (Math.abs(element.volume - session.volume) > 0.001)
      element.volume = session.volume;
    if (element.muted === session.soundEnabled)
      element.muted = !session.soundEnabled;
    const applyPosition = () => {
      if (Math.abs(element.currentTime - session.position) > 0.35) {
        try {
          element.currentTime = session.position;
        } catch {
          // An unsupported or not-yet-seekable source remains truthful in
          // its own media error state; playback ownership still stays unique.
        }
      }
      if (session.playing) {
        void element.play().catch((error) => {
          log.warn("media playback start failed", toErrorFields(error));
          emitPlayback("playback://observe", {
            surface,
            key,
            position: element.currentTime,
            playing: false,
            volume: element.volume,
            muted: element.muted,
          });
        });
      } else {
        element.pause();
      }
    };
    if (element.readyState >= HTMLMediaElement.HAVE_METADATA) applyPosition();
    else
      element.addEventListener("loadedmetadata", applyPosition, { once: true });
    return () => element.removeEventListener("loadedmetadata", applyPosition);
  }, [elementRef, enabled, key, session, surface]);

  const observe = useCallback(() => {
    if (mediaUseActive()) return;
    const element = elementRef.current;
    if (element === null) return;
    emitPlayback("playback://observe", {
      surface,
      key,
      position: element.currentTime,
      playing: !element.paused && !element.ended,
      volume: element.volume,
      muted: element.muted,
    });
  }, [elementRef, key, surface]);

  return {
    elementRef,
    ref: setElementRef,
    ownsPlayback: enabled && session?.owner === surface && session.key === key,
    onPlay: observe,
    onPause: observe,
    onTimeUpdate: observe,
    onVolumeChange: observe,
    onEnded: observe,
    toggle: () => emitPlayback("playback://toggle", { key }),
    seekAndPlay: (position: number) =>
      emitPlayback("playback://seek", { key, position, play: true }),
    seek: (position: number) =>
      emitPlayback("playback://seek", {
        key,
        position,
        play: session?.key === key ? session.playing : false,
      }),
  };
}
