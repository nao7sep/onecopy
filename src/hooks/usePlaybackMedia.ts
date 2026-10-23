import { useCallback, useEffect, useRef, useState } from "react";
import { emit } from "@tauri-apps/api/event";
import type {
  PlaybackMedium,
  PlaybackSession,
  PlaybackRegistration,
  PlaybackSurface,
} from "../models/playback";
import { mediaUseActive, useOwnedMedia } from "../media-use";
import { log, toErrorFields } from "../repositories";
import { recordActionFailure } from "../state/notifications-store";
import { message } from "../i18n/translate";
import {
  registerPlaybackClient,
  unregisterPlaybackClient,
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
  const cleanupApplication = useRef<(() => void) | null>(null);
  const restorePending = useRef(false);
  const suppressedSession = useRef<PlaybackSession | null>(null);
  const [elementRef, setElementRef] = useOwnedMedia<T>(key, (restorePlayback) => {
    suppressedSession.current = restorePlayback ? null : usePlaybackClientStore.getState().session;
    restorePending.current = true;
    applyCurrent();
  });
  const [setupFailed, setSetupFailed] = useState(false);
  const setupAttempt = useRef(0);
  const session = usePlaybackClientStore((state) => state.session);
  const register = useCallback(async () => {
    const attempt = ++setupAttempt.current;
    const registration: PlaybackRegistration = { surface, key, medium };
    try {
      await registerPlaybackClient(registration);
      if (setupAttempt.current === attempt) setSetupFailed(false);
    } catch {
      // The playback client preserves the original diagnostic. This hook owns
      // only the authored, per-media recovery result.
      if (setupAttempt.current === attempt) setSetupFailed(true);
    }
  }, [key, medium, surface]);

  useEffect(() => {
    const registration: PlaybackRegistration = { surface, key, medium };
    if (!enabled) {
      setSetupFailed(false);
      return;
    }
    void register();
    return () => {
      setupAttempt.current += 1;
      unregisterPlaybackClient(registration);
    };
  }, [enabled, key, medium, register, surface]);

  const applyCurrent = useCallback(() => {
    cleanupApplication.current?.();
    cleanupApplication.current = null;
    const element = elementRef.current;
    if (element === null || mediaUseActive()) return;
    const latest = usePlaybackClientStore.getState().session;
    const owns = enabled && latest?.owner === surface && latest.key === key;
    if (!owns) {
      restorePending.current = false;
      element.pause();
      return;
    }
    if (Math.abs(element.volume - latest.volume) > 0.001)
      element.volume = latest.volume;
    if (element.muted === latest.soundEnabled)
      element.muted = !latest.soundEnabled;
    let current = true;
    const stillCurrent = () => current && !mediaUseActive() && elementRef.current === element &&
      usePlaybackClientStore.getState().session === latest;
    const applyPosition = () => {
      if (!stillCurrent()) return;
      const restoring = restorePending.current;
      restorePending.current = false;
      let restorationFailed = false;
      const reportRestoreFailure = (error: unknown) => {
        if (restorationFailed) return;
        restorationFailed = true;
        log.error("playback restore failed", toErrorFields(error));
        recordActionFailure("playback-restore-failed", message("playback.restoreFailed"), error);
      };
      if (Math.abs(element.currentTime - latest.position) > 0.35) {
        try {
          element.currentTime = latest.position;
        } catch (error) {
          if (restoring) reportRestoreFailure(error);
          // An unsupported or not-yet-seekable source otherwise remains
          // truthful in its own media error state.
        }
      }
      if (latest.playing && suppressedSession.current !== latest) {
        void element.play().catch((error) => {
          if (!stillCurrent()) return;
          if (restoring) reportRestoreFailure(error);
          else log.warn("media playback start failed", toErrorFields(error));
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
    // Resume installs this listener before load(), which may still expose
    // the previous source’s readyState until that call resets the element.
    if (!restorePending.current && element.readyState >= HTMLMediaElement.HAVE_METADATA) applyPosition();
    else element.addEventListener("loadedmetadata", applyPosition, { once: true });
    cleanupApplication.current = () => {
      current = false;
      element.removeEventListener("loadedmetadata", applyPosition);
    };
  }, [elementRef, enabled, key, surface]);

  useEffect(() => {
    applyCurrent();
    return () => {
      cleanupApplication.current?.();
      cleanupApplication.current = null;
    };
  }, [applyCurrent, session]);

  const observe = useCallback(() => {
    if (mediaUseActive() || restorePending.current) return;
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
    setupFailed,
    retrySetup: register,
  };
}
