// ONE preview surface, two placements: the second-monitor preview window and
// the main window's split pane both render this. Images show the cached
// preview at fit (Z/click for the true 100% view); videos show the poster and
// strip with playback as an EXPLICIT act — auto-mounting <video> while the
// selection scrubs would read a 1 MiB original head-chunk per keystroke.
// Detail arrives as a prop (the anchor owner fetched it once); the surface
// itself fetches nothing.

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { hasOpenModal } from "../utils/modalStack";
import { log, toErrorFields } from "../repositories";
import { isEditableTarget } from "../utils/shortcuts";
import {
  isAudioFile,
  originalUrl,
  originalUrlByPath,
  previewUrl,
  stripUrl,
} from "../models/items";
import ZoomableImage from "./ZoomableImage";
import type { ItemDetail } from "../state/items-store";
import { Play } from "lucide-react";

function VideoSurface({ hash, detail }: { hash: string; detail: ItemDetail }) {
  const [playing, setPlaying] = useState(false);
  const [playbackFailed, setPlaybackFailed] = useState(false);
  const videoRef = useRef<HTMLVideoElement | null>(null);

  // The one exception in "Space = look": with a video loaded here, Space
  // plays/pauses it (the media convention) instead of closing the preview.
  // The store's shared Space rule DECLINES the key in that state, so this
  // listener is the only claimant left standing.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== " " || event.metaKey || event.ctrlKey || event.altKey) return;
      if (hasOpenModal()) return;
      if (isEditableTarget(event.target)) return;
      event.preventDefault();
      const video = videoRef.current;
      if (video === null) {
        setPlaying(true);
        return;
      }
      if (video.paused) void video.play();
      else video.pause();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return (
    <div className="flex h-full w-full flex-col items-center gap-2 overflow-y-auto">
      {playing && !playbackFailed ? (
        <video
          ref={videoRef}
          controls
          autoPlay
          poster={previewUrl(hash)}
          src={originalUrl(hash)}
          className="max-h-[70%] max-w-full"
          onError={() => setPlaybackFailed(true)}
        />
      ) : (
        <div className="relative flex max-h-[70%] items-center justify-center">
          <img
            src={previewUrl(hash)}
            alt={detail.fileName}
            className="max-h-full max-w-full object-contain"
          />
          {!playbackFailed ? (
            <button
              className="absolute whitespace-nowrap rounded-full bg-background/80 px-4 py-2 text-sm text-ink hover:bg-background"
              onClick={() => setPlaying(true)}
            >
              <Play size={14} className="mr-1 inline-block align-[-0.15em]" /> Play
            </button>
          ) : null}
        </div>
      )}
      <div className="flex flex-wrap justify-center gap-1">
        {Array.from({ length: detail.stripFrames ?? 0 }, (_, i) => (
          <img
            key={i}
            src={stripUrl(hash, i)}
            alt={`snapshot ${i + 1}`}
            loading="lazy"
            className="h-24 rounded-lg border border-border"
          />
        ))}
      </div>
      {playbackFailed ? (
        <p className="text-xs text-ink-muted">
          This codec does not play in the app's webview.
        </p>
      ) : null}
      <button
        className="inline-flex h-8 items-center justify-center rounded-lg border border-border px-3 text-sm font-medium text-ink transition-colors hover:border-border-strong hover:bg-surface-muted"
        onClick={() => {
          // The core resolves the hash to a live copy itself — the JS opener
          // route was scope-rejected into a silent no-op, so this button did
          // nothing for the exact user it exists for (unplayable codec).
          void invoke("open_item_externally", { hash }).catch((error) =>
            log.warn("open in player failed", toErrorFields(error)),
          );
        }}
      >
        Open in player
      </button>
    </div>
  );
}

/** An image whose missing/undecodable preview reads as words, never as the
 * webview's broken-image icon (a file the scan hasn't reached yet; a HEIC or
 * AVIF still waiting on the ffmpeg install that decodes it). */
function ImageSurface({
  hash,
  fileName,
  startZoomed = false,
}: {
  hash: string;
  fileName: string;
  startZoomed?: boolean;
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
      startZoomed={startZoomed}
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
  zoom = false,
}: {
  hash: string | null;
  detail: ItemDetail | null;
  /** The unhashed route: identifies the file when no hash exists yet. */
  pathId?: number | null;
  /** Enter's inspect: the image mounts at 100%. */
  zoom?: boolean;
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
        <VideoSurface key={hash} hash={hash} detail={detail} />
      ) : (
        <ImageSurface
          key={hash}
          hash={hash}
          fileName={detail?.fileName ?? ""}
          startZoomed={zoom}
        />
      )}
    </div>
  );
}
