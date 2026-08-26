// ONE preview surface, two placements: the second-monitor preview window and
// the main window's split pane both render this. Images show the cached
// preview at fit (Z/click for the true 100% view); videos share one player,
// scene rail, and transcript layout. Playback is delayed briefly so keyboard
// selection can settle without starting every video crossed along the way.
// Detail arrives as a prop (the anchor owner fetched it once); the surface
// itself fetches nothing.

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";
import { isEditableTarget } from "../utils/shortcuts";
import {
  isAudioFile,
  originalUrl,
  originalUrlByPath,
  previewUrl,
  stripTimestampMs,
  stripUrl,
  timestampLabel,
} from "../models/items";
import ZoomableImage from "./ZoomableImage";
import type { ItemDetail } from "../state/items-store";
import { ExternalLink } from "lucide-react";
import TranscriptBlock from "./TranscriptBlock";
import { useAppStore } from "../state/app-store";

function VideoSurface({
  hash,
  detail,
  seekMs,
  playAfterSeek,
  keyboardActive,
  autoplayImmediately,
}: {
  hash: string;
  detail: ItemDetail;
  seekMs?: number;
  playAfterSeek?: boolean;
  keyboardActive?: boolean;
  autoplayImmediately?: boolean;
}) {
  const [playbackFailed, setPlaybackFailed] = useState(false);
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const surfaceRef = useRef<HTMLDivElement | null>(null);
  const autoplayOnShow = useAppStore(
    (state) => state.appData?.config?.videoAutoplayOnShow !== false,
  );
  const autoplayAfterSnapshot = useAppStore(
    (state) => state.appData?.config?.videoAutoplayAfterSnapshot !== false,
  );
  const sceneCount = Math.max(0, detail.stripFrames ?? 0);

  useEffect(() => {
    if (keyboardActive) surfaceRef.current?.focus({ preventScroll: true });
  }, [keyboardActive]);

  const seek = (atMs: number, play: boolean) => {
    const video = videoRef.current;
    if (video === null) return;
    video.currentTime = atMs / 1000;
    if (play) void video.play().catch(() => undefined);
  };

  useEffect(() => {
    const video = videoRef.current;
    if (video === null || !autoplayOnShow || seekMs !== undefined) return;
    if (autoplayImmediately) {
      void video.play().catch(() => undefined);
      return;
    }
    const timer = setTimeout(() => {
      void video.play().catch(() => undefined);
    }, 250);
    return () => clearTimeout(timer);
  }, [autoplayImmediately, autoplayOnShow, hash, seekMs]);

  useEffect(() => {
    if (seekMs === undefined) return;
    const video = videoRef.current;
    if (video === null) return;
    const apply = () => seek(seekMs, playAfterSeek ?? autoplayAfterSnapshot);
    if (video.readyState >= HTMLMediaElement.HAVE_METADATA) apply();
    else video.addEventListener("loadedmetadata", apply, { once: true });
    return () => video.removeEventListener("loadedmetadata", apply);
  }, [autoplayAfterSnapshot, playAfterSeek, seekMs]);

  // A focused player owns Space; Quick View routing stands down here.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== " " || event.metaKey || event.ctrlKey || event.altKey) return;
      if (isEditableTarget(event.target)) return;
      if (!surfaceRef.current?.contains(document.activeElement)) return;
      event.preventDefault();
      const video = videoRef.current;
      if (video === null) return;
      if (video.paused) void video.play().catch(() => undefined);
      else video.pause();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
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
      <div className="relative min-h-0 flex-1 overflow-hidden rounded-lg bg-background">
        {playbackFailed ? (
          <img
            src={previewUrl(hash)}
            alt={detail.fileName}
            className="h-full w-full object-contain"
          />
        ) : (
          <video
            ref={videoRef}
            controls
            playsInline
            poster={previewUrl(hash)}
            src={originalUrl(hash)}
            className="h-full w-full object-contain"
            onError={() => setPlaybackFailed(true)}
          />
        )}
        <button
          className="absolute right-2 top-2 inline-flex h-8 items-center gap-1 rounded-lg bg-background/85 px-2.5 text-xs font-medium text-ink shadow-sm hover:bg-background"
          onClick={() => {
            void invoke("open_item_externally", { hash }).catch((error) =>
              log.warn("open in player failed", toErrorFields(error)),
            );
          }}
        >
          <ExternalLink size={13} /> Open in player
        </button>
        {sceneCount > 0 ? (
          <div className="absolute inset-x-2 bottom-10 flex gap-1 overflow-x-auto rounded-lg bg-background/75 p-1.5 backdrop-blur-sm">
            {Array.from({ length: sceneCount }, (_, index) => {
              const atMs = stripTimestampMs(detail.durationMs ?? 0, sceneCount, index);
              return (
                <button
                  key={index}
                  className="relative h-16 w-24 shrink-0 overflow-hidden rounded border border-border hover:border-primary-ring"
                  title={`Play from ${timestampLabel(atMs)}`}
                  aria-label={`Play from ${timestampLabel(atMs)}`}
                  onClick={() => seek(atMs, autoplayAfterSnapshot)}
                >
                  <img
                    src={stripUrl(hash, index)}
                    alt={`snapshot at ${timestampLabel(atMs)}`}
                    loading="lazy"
                    className="h-full w-full object-cover"
                  />
                  <span className="absolute bottom-0.5 right-0.5 rounded bg-background/80 px-1 text-[10px] text-ink">
                    {timestampLabel(atMs)}
                  </span>
                </button>
              );
            })}
          </div>
        ) : null}
      </div>
      {playbackFailed ? (
        <p className="shrink-0 text-xs text-ink-muted">
          This codec does not play in the app. Open it in your player instead.
        </p>
      ) : null}
      <div className="max-h-[35%] shrink-0 overflow-hidden">
        <TranscriptBlock hash={hash} />
      </div>
    </div>
  );
}

/** An image whose missing/undecodable preview reads as words, never as the
 * webview's broken-image icon (a file the scan hasn't reached yet; a HEIC or
 * AVIF still waiting on the ffmpeg install that decodes it). */
function ImageSurface({
  hash,
  fileName,
}: {
  hash: string;
  fileName: string;
}) {
  // A missing cache entry is USUALLY just a photo the scan's bulk pass has
  // not reached (it runs walk-order; on a slow machine the tail is hours
  // away), so the first failure asks the core to derive THIS one now and
  // retries once. Only when that also fails does the surface settle on words
  // — the core's own reason, which knows "install ffmpeg" from "broken file".
  const [phase, setPhase] = useState<
    { kind: "showing"; attempt: number; cacheHash: string }
    | { kind: "converting" }
    | { kind: "failed"; reason: string }
  >({ kind: "showing", attempt: 0, cacheHash: hash });
  if (phase.kind === "converting") {
    return <p className="text-sm text-ink-muted">Converting…</p>;
  }
  if (phase.kind === "failed") {
    return <p className="text-sm text-ink-muted">{phase.reason}</p>;
  }
  return (
    <ZoomableImage
      key={`${phase.cacheHash}-${phase.attempt}`}
      hash={phase.cacheHash}
      fileName={fileName}
      onError={() => {
        if (phase.attempt > 0) {
          setPhase({
            kind: "failed",
            reason: "No preview yet — not derived, or this format cannot be decoded.",
          });
          return;
        }
        setPhase({ kind: "converting" });
        invoke<string>("ensure_preview", { hash })
          .then((cacheHash) => setPhase({ kind: "showing", attempt: 1, cacheHash }))
          .catch((error) => {
            log.warn("on-demand preview derive failed", toErrorFields(error));
            setPhase({ kind: "failed", reason: String(error) });
          });
      }}
    />
  );
}

/** Audio other-files play instead of showing a blank surface (decided
 * 2026-08-16). The file stays an other-file everywhere else; the element
 * streams over the mediafile protocol — by hash when one exists, by path id
 * otherwise (a memo with a unique size is never content-read). */
function AudioSurface({
  src,
  fileName,
}: {
  src: string;
  fileName: string;
}) {
  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-3 p-4">
      <p className="max-w-full truncate text-sm text-ink" title={fileName}>
        {fileName}
      </p>
      {/* Keyed by src so playback state never carries across files. */}
      <audio key={src} controls src={src} className="w-full max-w-[420px]" />
    </div>
  );
}

export default function PreviewSurface({
  hash,
  detail,
  pathId = null,
  seekMs,
  playAfterSeek,
  keyboardActive = false,
  autoplayImmediately = false,
}: {
  hash: string | null;
  detail: ItemDetail | null;
  /** The unhashed route: identifies the file when no hash exists yet. */
  pathId?: number | null;
  /** A scene rail can open the player at this exact point. */
  seekMs?: number;
  /** Overrides the snapshot autoplay preference for this one navigation. */
  playAfterSeek?: boolean;
  /** A transient owning layer can give its player the media keys. */
  keyboardActive?: boolean;
  /** Explicit views may play now; selection-follow Preview stays debounced. */
  autoplayImmediately?: boolean;
}) {
  if (detail !== null && isAudioFile(detail.fileName)) {
    const src =
      hash !== null ? originalUrl(hash) : pathId !== null ? originalUrlByPath(pathId) : null;
    if (src !== null) {
      return <AudioSurface src={src} fileName={detail.fileName} />;
    }
  }
  if (hash === null && detail === null) {
    return (
      <div className="flex h-full items-center justify-center">
        <p className="text-ink-muted">Select an item</p>
      </div>
    );
  }
  if (hash === null) {
    return (
      <div className="flex h-full items-center justify-center">
        <p className="text-ink-muted">{detail?.fileName ?? ""} (no preview)</p>
      </div>
    );
  }
  return (
    <div className="flex h-full min-h-0 items-center justify-center overflow-hidden p-2">
      {detail?.kind === "video" ? (
        // Keyed by hash so playback state never carries across files.
        <VideoSurface
          key={hash}
          hash={hash}
          detail={detail}
          seekMs={seekMs}
          playAfterSeek={playAfterSeek}
          keyboardActive={keyboardActive}
          autoplayImmediately={autoplayImmediately}
        />
      ) : (
        <ImageSurface
          key={hash}
          hash={hash}
          fileName={detail?.fileName ?? ""}
        />
      )}
    </div>
  );
}
