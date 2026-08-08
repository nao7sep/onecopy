import { useEffect, useState } from "react";
import { listen, emit } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openPath } from "@tauri-apps/plugin-opener";
import { previewUrl, stripUrl } from "../models/items";
import type { ItemDetail } from "../state/items-store";
import type { PreviewPayload } from "../state/preview-store";

// The preview window's content: the cached screen-fit preview for images (and
// video posters), the snapshot strip plus an "Open in player" fallback for
// videos (in-webview playback is a separate task — it needs a range-capable
// protocol for seeking). Follows the main window's selection via
// `preview://show`; Escape closes.

export default function PreviewWindow() {
  const [payload, setPayload] = useState<PreviewPayload | null>(null);
  const [detail, setDetail] = useState<ItemDetail | null>(null);

  useEffect(() => {
    const unlisten = listen<PreviewPayload>("preview://show", (event) => {
      setPayload(event.payload);
    });
    // Ask the main window for the current selection on load, so an
    // already-made selection appears without waiting for the next change.
    void emit("preview://ready", {});
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        void getCurrentWindow().close();
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
      <div className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto p-2">
        {detail.kind === "video" && hash !== null ? (
          <div className="flex h-full w-full flex-col items-center gap-2 overflow-y-auto">
            <img
              src={previewUrl(hash)}
              alt={detail.fileName}
              className="max-h-[60%] max-w-full object-contain"
            />
            <div className="flex flex-wrap justify-center gap-1">
              {Array.from({ length: detail.stripFrames ?? 0 }, (_, i) => (
                <img
                  key={i}
                  src={stripUrl(hash, i)}
                  alt={`snapshot ${i + 1}`}
                  loading="lazy"
                  className="h-28 rounded border border-border"
                />
              ))}
            </div>
            <button
              className="rounded bg-primary px-3 py-1 text-sm text-ink-inverted"
              onClick={() => {
                const path = detail.copyPaths[0];
                if (path) void openPath(path);
              }}
            >
              Open in player
            </button>
          </div>
        ) : hash !== null ? (
          <img
            src={previewUrl(hash)}
            alt={detail.fileName}
            className="max-h-full max-w-full object-contain"
          />
        ) : (
          <p className="text-ink-muted">{detail.fileName} (no preview)</p>
        )}
      </div>
      <footer className="flex shrink-0 justify-between border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
        <span className="truncate" title={detail.fileName}>
          {detail.fileName}
        </span>
        <span>Escape closes</span>
      </footer>
    </div>
  );
}
