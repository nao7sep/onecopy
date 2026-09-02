// ONE preview surface, two placements: the second-monitor preview window and
// the main window's split pane both render this. Images show the cached
// preview at fit (press and hold for original pixels); video, audio, text, and
// attributes retain distinct bodies while composing shared session mechanics.
// Detail arrives as a prop from the anchor owner; bounded text and explicit
// on-demand preparation are the only body-owned reads.

import { useEffect, useRef, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";
import {
  isAudioFile,
  originalUrl,
  originalUrlByPath,
  previewUrl,
  stripTimestampMs,
  stripUrl,
  timestampLabel,
  formatBytes,
} from "../models/items";
import InspectableImage, {
  inspectPosition,
  intrinsicOffset,
  type InspectPosition,
} from "./InspectableImage";
import { useHoldInspect, type PointerPoint } from "../hooks/useHoldInspect";
import type { ItemDetail } from "../models/items";
import type { PlaybackSurface } from "../models/playback";
import { ExternalLink, Pause, Play, Volume2, VolumeX } from "lucide-react";
import TranscriptBlock from "./TranscriptBlock";
import { usePlaybackMedia } from "../hooks/usePlaybackMedia";
import { useAppStore } from "../state/app-store";
import TextOrAttributesSurface from "./TextOrAttributesSurface";
import { openInDefaultApp } from "../workflows/external-open";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";

function VideoSurface({
  hash,
  detail,
  surface,
  keyboardActive,
}: {
  hash: string;
  detail: ItemDetail;
  surface: PlaybackSurface;
  keyboardActive?: boolean;
}) {
  const [playbackFailed, setPlaybackFailed] = useState(false);
  const [externalError, setExternalError] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const [position, setPosition] = useState(0);
  const [duration, setDuration] = useState(
    Math.max(0, (detail.durationMs ?? 0) / 1000),
  );
  const [muted, setMuted] = useState(false);
  const [volume, setVolume] = useState(1);
  const [inspectPositionState, setInspectPositionState] =
    useState<InspectPosition>({
      x: 0.5,
      y: 0.5,
      width: 0,
      height: 0,
    });
  const playback = usePlaybackMedia<HTMLVideoElement>(
    surface,
    hash,
    "video",
    !playbackFailed,
  );
  const videoRef = playback.elementRef;
  const surfaceRef = useRef<HTMLDivElement | null>(null);
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const resumeAfterInspectRef = useRef(false);
  const inspectingRef = useRef(false);
  const sceneCount = Math.max(0, detail.stripFrames ?? 0);
  const updateInspectPosition = (point: PointerPoint) => {
    setInspectPositionState(inspectPosition(viewportRef.current, point));
  };
  const hold = useHoldInspect({
    onStart: (point) => {
      const video = videoRef.current;
      inspectingRef.current = true;
      resumeAfterInspectRef.current = video !== null && !video.paused;
      video?.pause();
      updateInspectPosition(point);
    },
    onMove: updateInspectPosition,
    onEnd: () => {
      inspectingRef.current = false;
      if (resumeAfterInspectRef.current) {
        void videoRef.current?.play().catch(() => undefined);
      }
      resumeAfterInspectRef.current = false;
    },
  });

  useEffect(() => {
    if (keyboardActive) surfaceRef.current?.focus({ preventScroll: true });
  }, [keyboardActive]);

  return (
    <div
      ref={surfaceRef}
      data-video-surface
      tabIndex={keyboardActive ? 0 : -1}
      role={keyboardActive ? "group" : undefined}
      aria-label={keyboardActive ? "Video Quick View" : undefined}
      className="flex h-full min-h-0 w-full flex-col gap-2 outline-none"
    >
      <div
        ref={viewportRef}
        className={`relative min-h-0 flex-1 overflow-hidden rounded-lg bg-background ${
          hold.inspecting ? "cursor-crosshair" : ""
        }`}
        title={
          playbackFailed
            ? undefined
            : "Press and hold the picture for original pixels"
        }
        onPointerDown={playbackFailed ? undefined : hold.onPointerDown}
        onClickCapture={playbackFailed ? undefined : hold.onClickCapture}
      >
        {playbackFailed ? (
          <InspectableImage
            hash={hash}
            fileName={detail.fileName}
            enlargeSmall
          />
        ) : (
          <video
            ref={playback.ref}
            playsInline
            poster={previewUrl(hash)}
            src={originalUrl(hash)}
            className={
              hold.inspecting
                ? "pointer-events-none absolute max-h-none max-w-none"
                : "h-full w-full object-contain"
            }
            style={
              hold.inspecting
                ? videoInspectStyle(
                    videoRef.current,
                    detail,
                    inspectPositionState,
                  )
                : undefined
            }
            onError={() => setPlaybackFailed(true)}
            onClick={() => playback.toggle()}
            onLoadedMetadata={(event) =>
              setDuration(event.currentTarget.duration)
            }
            onPlay={() => {
              setPlaying(true);
              playback.onPlay();
            }}
            onPause={() => {
              setPlaying(false);
              playback.onPause();
            }}
            onTimeUpdate={(event) => {
              setPosition(event.currentTarget.currentTime);
              playback.onTimeUpdate();
            }}
            onVolumeChange={(event) => {
              setMuted(event.currentTarget.muted);
              setVolume(event.currentTarget.volume);
              playback.onVolumeChange();
            }}
            onEnded={() => {
              setPlaying(false);
              playback.onEnded();
            }}
          />
        )}
        {!hold.inspecting && !playbackFailed ? (
          <button
            className="absolute right-2 top-2 inline-flex h-8 items-center gap-1 rounded-lg bg-background/85 px-2.5 text-xs font-medium text-ink shadow-sm hover:bg-background"
            onPointerDown={(event) => event.stopPropagation()}
            onClick={() => {
              setExternalError(null);
              void openInDefaultApp(hash, null).catch((error) => {
                log.warn("open in player failed", toErrorFields(error));
                setExternalError(
                  "Couldn’t open this video in the external player.",
                );
              });
            }}
          >
            <ExternalLink size={13} /> Open in player
          </button>
        ) : null}
        {!hold.inspecting && sceneCount > 0 ? (
          <div
            className="absolute inset-x-2 bottom-12 flex gap-1 overflow-x-auto rounded-lg bg-background/75 p-1.5 backdrop-blur-sm"
            onPointerDown={(event) => event.stopPropagation()}
          >
            {Array.from({ length: sceneCount }, (_, index) => {
              const atMs = stripTimestampMs(
                detail.durationMs ?? 0,
                sceneCount,
                index,
              );
              return (
                <button
                  key={index}
                  className="relative h-16 w-24 shrink-0 overflow-hidden rounded border border-border hover:border-primary-ring"
                  title={`Play from ${timestampLabel(atMs)}`}
                  aria-label={`Play from ${timestampLabel(atMs)}`}
                  onClick={() => playback.seekAndPlay(atMs / 1000)}
                >
                  <img
                    src={stripUrl(hash, index)}
                    alt={`snapshot at ${timestampLabel(atMs)}`}
                    loading="lazy"
                    className="h-full w-full object-contain"
                  />
                  <span className="absolute bottom-0.5 right-0.5 rounded bg-background/80 px-1 text-[11px] text-ink">
                    {timestampLabel(atMs)}
                  </span>
                </button>
              );
            })}
          </div>
        ) : null}
        {!hold.inspecting && !playbackFailed ? (
          <div
            className="absolute inset-x-2 bottom-2 flex h-9 items-center gap-2 rounded-lg bg-background/85 px-2 backdrop-blur-sm"
            onPointerDown={(event) => event.stopPropagation()}
          >
            <button
              className="rounded p-1 text-ink hover:bg-surface-muted"
              aria-label={playing ? "Pause" : "Play"}
              onClick={() => playback.toggle()}
            >
              {playing ? <Pause size={16} /> : <Play size={16} />}
            </button>
            <span className="w-10 text-right font-mono text-[11px] text-ink-muted">
              {timestampLabel(position * 1000)}
            </span>
            <input
              aria-label="Playback position"
              type="range"
              min={0}
              max={Math.max(duration, 0.01)}
              step={0.1}
              value={Math.min(position, Math.max(duration, 0.01))}
              className="min-w-16 flex-1"
              onChange={(event) => playback.seek(Number(event.target.value))}
            />
            <span className="w-10 font-mono text-[11px] text-ink-muted">
              {timestampLabel(duration * 1000)}
            </span>
            <button
              className="rounded p-1 text-ink hover:bg-surface-muted"
              aria-label={muted ? "Turn sound on" : "Turn sound off"}
              onClick={() => {
                const video = videoRef.current;
                if (video !== null) video.muted = !video.muted;
              }}
            >
              {muted ? <VolumeX size={16} /> : <Volume2 size={16} />}
            </button>
            <input
              aria-label="Playback volume"
              type="range"
              min={0.01}
              max={1}
              step={0.01}
              value={volume}
              className="w-20"
              onChange={(event) => {
                const video = videoRef.current;
                if (video === null) return;
                video.volume = Number(event.target.value);
                video.muted = false;
              }}
            />
          </div>
        ) : null}
      </div>
      {playbackFailed ? (
        <p className="shrink-0 text-xs text-ink-muted">
          This codec does not play in the app.
          {detail.byteSize !== null ? ` ${formatBytes(detail.byteSize)}` : ""}
          {detail.width !== null && detail.height !== null
            ? ` · ${detail.width}×${detail.height}`
            : ""}
          {` · ${detail.copyPaths.length.toLocaleString()} ${detail.copyPaths.length === 1 ? "copy" : "copies"}`}
        </p>
      ) : null}
      {externalError !== null ? (
        <OperationResult level="error" className="shrink-0">
          {externalError}
        </OperationResult>
      ) : null}
      {playback.setupFailed ? (
        <OperationResult
          level="error"
          className="shrink-0"
          actions={<Button variant="ghost" onClick={() => void playback.retrySetup()}>Retry</Button>}
        >
          Playback controls could not be connected. Try again.
        </OperationResult>
      ) : null}
      {playbackFailed ? (
        <div className="max-h-[35%] min-h-28 shrink-0 overflow-hidden">
          <TextOrAttributesSurface
            hash={hash}
            pathId={null}
            detail={detail}
            specializedFailure="Built-in video playback failed."
          />
        </div>
      ) : null}
      <div className="max-h-[35%] shrink-0 overflow-hidden">
        <TranscriptBlock hash={hash} medium="video" />
      </div>
    </div>
  );
}

function videoInspectStyle(
  video: HTMLVideoElement | null,
  detail: ItemDetail,
  position: InspectPosition,
): CSSProperties {
  const width = video?.videoWidth || detail.width || 0;
  const height = video?.videoHeight || detail.height || 0;
  if (
    width <= 0 ||
    height <= 0 ||
    position.width <= 0 ||
    position.height <= 0
  ) {
    return { visibility: "hidden" };
  }
  return {
    width: `${width}px`,
    height: `${height}px`,
    left: `${intrinsicOffset(width, position.width, position.x)}px`,
    top: `${intrinsicOffset(height, position.height, position.y)}px`,
  };
}

/** An image whose missing/undecodable preview reads as words, never as the
 * webview's broken-image icon (a file the scan hasn't reached yet; a HEIC or
 * AVIF still waiting on the ffmpeg install that decodes it). */
function ImageSurface({
  hash,
  detail,
  enlargeSmall,
}: {
  hash: string;
  detail: ItemDetail;
  enlargeSmall: boolean;
}) {
  const [externalError, setExternalError] = useState<string | null>(null);
  // A missing cache entry is USUALLY just a photo the scan's bulk pass has
  // not reached (it runs walk-order; on a slow machine the tail is hours
  // away), so the first failure asks the core to derive THIS one now and
  // retries once. Only when that also fails does the surface settle on words
  // — the core's own reason, which knows "install ffmpeg" from "broken file".
  const [phase, setPhase] = useState<
    | { kind: "showing"; attempt: number; cacheHash: string }
    | { kind: "converting" }
    | { kind: "failed"; reason: string }
  >({ kind: "showing", attempt: 0, cacheHash: hash });
  if (phase.kind === "converting") {
    return <p className="text-sm text-ink-muted">Converting…</p>;
  }
  if (phase.kind === "failed") {
    return (
      <TextOrAttributesSurface
        hash={hash}
        pathId={null}
        detail={detail}
        specializedFailure={`Built-in image preview failed: ${phase.reason}`}
      />
    );
  }
  return (
    <div className="relative h-full w-full">
      <InspectableImage
        key={`${phase.cacheHash}-${phase.attempt}`}
        hash={phase.cacheHash}
        fileName={detail.fileName}
        enlargeSmall={enlargeSmall}
        sourceWidth={detail.width}
        sourceHeight={detail.height}
        onError={() => {
          if (phase.attempt > 0) {
            setPhase({
              kind: "failed",
              reason:
                "No preview yet — not derived, or this format cannot be decoded.",
            });
            return;
          }
          setPhase({ kind: "converting" });
          invoke<string>("ensure_preview", { hash })
            .then((cacheHash) =>
              setPhase({ kind: "showing", attempt: 1, cacheHash }),
            )
            .catch((error) => {
              log.warn("on-demand preview derive failed", toErrorFields(error));
              setPhase({
                kind: "failed",
                reason: "A preview could not be prepared for this file. File actions and known details remain available.",
              });
            });
        }}
      />
      <button
        className="absolute right-2 top-2 inline-flex h-8 items-center gap-1 rounded-lg bg-background/85 px-2.5 text-xs font-medium text-ink shadow-sm hover:bg-background"
        onClick={() => {
          setExternalError(null);
          void openInDefaultApp(hash, null).catch((error) => {
            log.warn("external image open failed", toErrorFields(error));
            setExternalError("Couldn’t open this image in its default app.");
          });
        }}
      >
        <ExternalLink size={13} /> Open in default app
      </button>
      {externalError !== null ? (
        <OperationResult
          level="error"
          className="absolute bottom-2 left-2 right-2 shadow-sm"
        >
          {externalError}
        </OperationResult>
      ) : null}
    </div>
  );
}

/** Audio keeps its own Other-file presentation while composing the shared
 * playback session. The element streams by content identity when complete and
 * by its indexed path id while that identity is still provisional. */
function AudioSurface({
  hash,
  src,
  detail,
  playbackKey,
  surface,
  pathId,
}: {
  hash: string | null;
  src: string;
  detail: ItemDetail;
  playbackKey: string;
  surface: PlaybackSurface;
  pathId: number | null;
}) {
  const [externalError, setExternalError] = useState<string | null>(null);
  const [playbackFailed, setPlaybackFailed] = useState(false);
  const playback = usePlaybackMedia<HTMLAudioElement>(
    surface,
    playbackKey,
    "audio",
    !playbackFailed,
  );
  if (playbackFailed) {
    return (
      <TextOrAttributesSurface
        hash={hash}
        pathId={pathId}
        detail={detail}
        specializedFailure="Built-in audio playback failed."
      />
    );
  }
  return (
    <div className="flex h-full min-h-0 w-full flex-col justify-center gap-5 p-4">
      <div className="flex shrink-0 flex-col items-center gap-3">
        <p
          className="max-w-full truncate text-sm text-ink"
          title={detail.fileName}
        >
          {detail.fileName}
        </p>
        {/* Keyed by src so playback state never carries across files. */}
        <audio
          ref={playback.ref}
          key={src}
          controls
          src={src}
          className="w-full max-w-[420px]"
          onPlay={playback.onPlay}
          onPause={playback.onPause}
          onTimeUpdate={playback.onTimeUpdate}
          onVolumeChange={playback.onVolumeChange}
          onEnded={playback.onEnded}
          onError={() => setPlaybackFailed(true)}
        />
        <p className="text-xs text-ink-muted">
          {detail.byteSize === null
            ? "Unknown size"
            : formatBytes(detail.byteSize)}
          {detail.durationMs === null
            ? ""
            : ` · ${timestampLabel(detail.durationMs)}`}
          {` · ${detail.copyPaths.length.toLocaleString()} ${detail.copyPaths.length === 1 ? "copy" : "copies"}`}
        </p>
        <Button
          variant="ghost"
          onClick={() => {
            setExternalError(null);
            void openInDefaultApp(hash, pathId).catch((error) => {
              log.warn("external audio open failed", toErrorFields(error));
              setExternalError(
                "Couldn’t open this audio file in its default app.",
              );
            });
          }}
        >
          <ExternalLink size={13} /> Open in default app
        </Button>
        {externalError !== null ? (
          <OperationResult level="error">{externalError}</OperationResult>
        ) : null}
        {playback.setupFailed ? (
          <OperationResult
            level="error"
            actions={<Button variant="ghost" onClick={() => void playback.retrySetup()}>Retry</Button>}
          >
            Playback controls could not be connected. Try again.
          </OperationResult>
        ) : null}
      </div>
      {hash !== null ? (
        <div className="max-h-[45%] shrink-0 overflow-hidden">
          <TranscriptBlock hash={hash} medium="audio" />
        </div>
      ) : null}
    </div>
  );
}

export default function PreviewSurface({
  hash,
  detail,
  pathId = null,
  surface,
  keyboardActive = false,
}: {
  hash: string | null;
  detail: ItemDetail | null;
  /** The unhashed route: identifies the file when no hash exists yet. */
  pathId?: number | null;
  surface: PlaybackSurface;
  /** A transient owning layer can give its player the media keys. */
  keyboardActive?: boolean;
}) {
  const enlargeSmall = useAppStore((state) => {
    const config = state.appData?.config;
    return surface === "quick" || surface === "viewer"
      ? config?.enlargeSmallImagesInQuickView !== false
      : config?.enlargeSmallImagesInPreview !== false;
  });
  if (detail !== null && isAudioFile(detail.fileName)) {
    const src =
      hash !== null
        ? originalUrl(hash)
        : pathId !== null
          ? originalUrlByPath(pathId)
          : null;
    if (src !== null) {
      return (
        <AudioSurface
          key={hash ?? `path-${pathId}`}
          hash={hash}
          src={src}
          detail={detail}
          playbackKey={hash ?? `path-${pathId}`}
          surface={surface}
          pathId={hash === null ? pathId : null}
        />
      );
    }
  }
  if (hash === null && detail === null) {
    return (
      <div className="flex h-full items-center justify-center">
        <p className="text-ink-muted">Select an item</p>
      </div>
    );
  }
  if (detail === null) {
    return (
      <div className="flex h-full items-center justify-center">
        <p className="text-ink-muted">Loading preview…</p>
      </div>
    );
  }
  if (detail !== null && detail.kind !== "image" && detail.kind !== "video") {
    return (
      <TextOrAttributesSurface
        hash={hash}
        pathId={hash === null ? pathId : null}
        detail={detail}
      />
    );
  }
  if (hash === null && detail !== null) {
    return (
      <TextOrAttributesSurface hash={null} pathId={pathId} detail={detail} />
    );
  }
  if (hash === null) return null;
  return (
    <div className="flex h-full min-h-0 items-center justify-center overflow-hidden p-2">
      {detail.kind === "video" ? (
        // Keyed by hash so playback state never carries across files.
        <VideoSurface
          key={hash}
          hash={hash}
          detail={detail}
          surface={surface}
          keyboardActive={keyboardActive}
        />
      ) : (
        <ImageSurface
          key={hash}
          hash={hash}
          detail={detail}
          enlargeSmall={enlargeSmall}
        />
      )}
    </div>
  );
}
