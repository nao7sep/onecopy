// ONE preview surface, two placements: the second-monitor preview window and
// the main window's split pane both render this. Images show the cached
// preview at fit (Z/click for the true 100% view); videos show the poster and
// strip with playback as an EXPLICIT act — auto-mounting <video> while the
// selection scrubs would read a 1 MiB original head-chunk per keystroke.
// Detail arrives as a prop (the anchor owner fetched it once); the surface
// itself fetches nothing.

import { useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import { originalUrl, previewUrl, stripUrl } from "../models/items";
import ZoomableImage from "./ZoomableImage";
import type { ItemDetail } from "../state/items-store";

function VideoSurface({ hash, detail }: { hash: string; detail: ItemDetail }) {
  const [playing, setPlaying] = useState(false);
  const [playbackFailed, setPlaybackFailed] = useState(false);
  return (
    <div className="flex h-full w-full flex-col items-center gap-2 overflow-y-auto">
      {playing && !playbackFailed ? (
        <video
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
              className="absolute rounded-full bg-background/80 px-4 py-2 text-sm text-ink hover:bg-background"
              onClick={() => setPlaying(true)}
            >
              ▶ Play
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
            className="h-24 rounded border border-border"
          />
        ))}
      </div>
      {playbackFailed ? (
        <p className="text-xs text-ink-muted">
          This codec does not play in the app's webview.
        </p>
      ) : null}
      <button
        className="rounded border border-border px-3 py-1 text-sm text-primary hover:bg-primary-surface"
        onClick={() => {
          const path = detail.copyPaths[0];
          if (path) void openPath(path);
        }}
      >
        Open in player
      </button>
    </div>
  );
}

/** An image whose missing/undecodable preview reads as words, never as the
 * webview's broken-image icon (HEIC before its decoder lands; a not-yet-
 * derived file the scan hasn't reached). */
function ImageSurface({ hash, fileName }: { hash: string; fileName: string }) {
  const [failed, setFailed] = useState(false);
  if (failed) {
    return (
      <p className="text-sm text-ink-muted">
        No preview yet — not derived, or this format cannot be decoded.
      </p>
    );
  }
  return <ZoomableImage key={hash} hash={hash} fileName={fileName} onError={() => setFailed(true)} />;
}

export default function PreviewSurface({
  hash,
  detail,
}: {
  hash: string | null;
  detail: ItemDetail | null;
}) {
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
        <ImageSurface key={hash} hash={hash} fileName={detail?.fileName ?? ""} />
      )}
    </div>
  );
}
