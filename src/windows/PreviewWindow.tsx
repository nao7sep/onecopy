import { useEffect, useRef, useState } from "react";
import { listen, emit } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openPath } from "@tauri-apps/plugin-opener";
import { originalUrl, previewUrl, stripUrl } from "../models/items";
import type { ItemDetail } from "../state/items-store";
import type { PreviewPayload } from "../state/preview-store";

// The preview window: images at fit (cached preview) with a click/Z toggle
// into the true 100% view (original bytes via the range-capable mediafile
// protocol, drag to pan); videos play in-webview through the same protocol
// (poster from the cache, system-player fallback for codecs the webview
// refuses); F toggles fullscreen; Escape leaves fullscreen, then closes.
// Follows the main window's selection via `preview://show`.

function ImageSurface({ hash, fileName }: { hash: string; fileName: string }) {
  const [zoomed, setZoomed] = useState(false);
  const panRef = useRef<HTMLDivElement | null>(null);
  const dragState = useRef<{ x: number; y: number; left: number; top: number } | null>(null);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() === "z") {
        event.preventDefault();
        setZoomed((z) => !z);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  if (!zoomed) {
    return (
      <img
        src={previewUrl(hash)}
        alt={fileName}
        title="Click or press Z for 100%"
        className="max-h-full max-w-full cursor-zoom-in object-contain"
        onClick={() => setZoomed(true)}
      />
    );
  }
  return (
    <div
      ref={panRef}
      className="h-full w-full cursor-grab overflow-auto active:cursor-grabbing"
      onMouseDown={(e) => {
        const pane = panRef.current;
        if (!pane) return;
        dragState.current = {
          x: e.clientX,
          y: e.clientY,
          left: pane.scrollLeft,
          top: pane.scrollTop,
        };
      }}
      onMouseMove={(e) => {
        const pane = panRef.current;
        const drag = dragState.current;
        if (!pane || !drag || e.buttons === 0) return;
        pane.scrollLeft = drag.left - (e.clientX - drag.x);
        pane.scrollTop = drag.top - (e.clientY - drag.y);
      }}
      onMouseUp={() => (dragState.current = null)}
      onDoubleClick={() => setZoomed(false)}
    >
      <img
        src={originalUrl(hash)}
        alt={fileName}
        title="Drag to pan · double-click or Z to fit"
        className="max-w-none"
        draggable={false}
      />
    </div>
  );
}

function VideoSurface({ hash, detail }: { hash: string; detail: ItemDetail }) {
  const [playbackFailed, setPlaybackFailed] = useState(false);
  return (
    <div className="flex h-full w-full flex-col items-center gap-2 overflow-y-auto">
      {playbackFailed ? (
        <img
          src={previewUrl(hash)}
          alt={detail.fileName}
          className="max-h-[60%] max-w-full object-contain"
        />
      ) : (
        <video
          controls
          preload="metadata"
          poster={previewUrl(hash)}
          src={originalUrl(hash)}
          className="max-h-[70%] max-w-full"
          onError={() => setPlaybackFailed(true)}
        />
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

export default function PreviewWindow() {
  const [payload, setPayload] = useState<PreviewPayload | null>(null);
  const [detail, setDetail] = useState<ItemDetail | null>(null);

  useEffect(() => {
    const unlisten = listen<PreviewPayload>("preview://show", (event) => {
      setPayload(event.payload);
    });
    // Ask the main window for the current selection on load.
    void emit("preview://ready", {});
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() === "f") {
        event.preventDefault();
        void (async () => {
          const window = getCurrentWindow();
          const full = await window.isFullscreen().catch(() => false);
          await window.setFullscreen(!full).catch(() => {});
        })();
      } else if (event.key === "Escape") {
        event.preventDefault();
        void (async () => {
          const window = getCurrentWindow();
          const full = await window.isFullscreen().catch(() => false);
          if (full) await window.setFullscreen(false).catch(() => {});
          else await window.close();
        })();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      void unlisten.then((fn) => fn());
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  useEffect(() => {
    setDetail(null);
    if (payload === null || (payload.hash === null && payload.pathId === null)) return;
    void invoke<ItemDetail>("get_item_detail", {
      hash: payload.hash,
      pathId: payload.hash === null ? payload.pathId : null,
    })
      .then(setDetail)
      .catch(() => setDetail(null));
  }, [payload]);

  if (payload === null || detail === null) {
    return (
      <div className="flex h-screen items-center justify-center bg-background">
        <p className="text-ink-muted">Select an item in the main window</p>
      </div>
    );
  }

  const hash = payload.hash;
  return (
    <div className="flex h-screen flex-col bg-background">
      <div className="flex min-h-0 flex-1 items-center justify-center overflow-hidden p-2">
        {detail.kind === "video" && hash !== null ? (
          <VideoSurface hash={hash} detail={detail} />
        ) : hash !== null ? (
          <ImageSurface hash={hash} fileName={detail.fileName} />
        ) : (
          <p className="text-ink-muted">{detail.fileName} (no preview)</p>
        )}
      </div>
      <footer className="flex shrink-0 justify-between border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
        <span className="truncate" title={detail.fileName}>
          {detail.fileName}
        </span>
        <span>Z: 100% · F: fullscreen · Escape: close</span>
      </footer>
    </div>
  );
}
