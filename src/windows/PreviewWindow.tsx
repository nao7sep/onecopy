import { useEffect, useState } from "react";
import { listenThenAnnounce } from "../utils/handshake";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { currentMonitor } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import PreviewSurface from "../components/PreviewSurface";
import type { PreviewShowMessage } from "../state/preview-store";
import { reportWindowCall } from "../repositories";

// The preview window placement (screen 2): renders the shared PreviewSurface
// from `preview://show` messages — payload AND detail arrive together from
// the anchor owner, so this window queries nothing and can never race a
// stale response. The previous message keeps rendering until the next one
// arrives (no blank flash between keystrokes). Library commands go back to
// Main, which remains their one owner.

export default function PreviewWindow() {
  const [message, setMessage] = useState<PreviewShowMessage | null>(null);

  // This window's own memory (Phase 33): geometry persists via patch_state —
  // written HERE because only this window sees its own moves. Maximized is a
  // flag, never geometry, so un-maximizing has a real size to return to. The
  // main window's ensurePreviewWindow reads these at the NEXT LAUNCH (its
  // in-memory state is boot-time); within a session the hidden-not-closed
  // window keeps its geometry live by itself.
  useEffect(() => {
    const window = getCurrentWindow();
    let timer: ReturnType<typeof setTimeout> | null = null;
    const save = () => {
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(() => {
        void (async () => {
          try {
            if (await window.isMaximized()) {
              await invoke("patch_state", { patch: { previewWindowMaximized: true } });
              return;
            }
            const position = await window.outerPosition();
            const size = await window.innerSize();
            await invoke("patch_state", {
              patch: {
                previewWindowMaximized: false,
                previewWindowBounds: {
                  x: position.x,
                  y: position.y,
                  width: size.width,
                  height: size.height,
                },
              },
            });
          } catch (error) {
            reportWindowCall("preview save bounds")(error);
          }
        })();
      }, 500);
    };
    const unlistens: Array<() => void> = [];
    void window.onMoved(save).then((fn) => unlistens.push(fn));
    void window.onResized(save).then((fn) => unlistens.push(fn));
    return () => {
      if (timer !== null) clearTimeout(timer);
      for (const fn of unlistens) fn();
    };
  }, []);

  useEffect(() => {
    // Ask the main window for the current selection — only once this window
    // can actually hear the reply (see handshake.ts).
    const unlisten = listenThenAnnounce<PreviewShowMessage>(
      "preview://show",
      "preview://ready",
      setMessage,
    );
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      if (event.key === " ") {
        // Persistent Preview never reinterprets Space as playback or as a
        // second transient-viewer toggle.
        event.preventDefault();
        event.stopPropagation();
      } else if (event.key.toLowerCase() === "f") {
        event.preventDefault();
        event.stopPropagation();
        void currentMonitor()
          .then((monitor) => emit("preview://fullscreen", monitor))
          .catch(reportWindowCall("preview current monitor"));
      } else if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        void getCurrentWindow().close().catch(reportWindowCall("preview close"));
      } else if (
        [
          "ArrowLeft",
          "ArrowRight",
          "ArrowUp",
          "ArrowDown",
          "PageUp",
          "PageDown",
          "Home",
          "End",
          "Enter",
          "Delete",
          "Backspace",
        ].includes(event.key)
      ) {
        if (
          event.target instanceof Element &&
          event.target.closest("button, input, select, textarea, [contenteditable='true']") !== null
        ) {
          return;
        }
        event.preventDefault();
        event.stopPropagation();
        void emit("preview://key", {
          key: event.key,
          code: event.code,
          shiftKey: event.shiftKey,
          metaKey: event.metaKey,
          ctrlKey: event.ctrlKey,
          altKey: event.altKey,
        }).catch(reportWindowCall("preview key forward"));
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      unlisten();
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, []);

  if (message === null) {
    return (
      <div className="flex h-screen items-center justify-center bg-background">
        <p className="text-ink-muted">Select an item in the main window</p>
      </div>
    );
  }

  return (
    <div className="flex h-screen flex-col bg-background">
      <div className="min-h-0 flex-1">
        <PreviewSurface
          hash={message.hash}
          detail={message.detail}
          pathId={message.pathId}
          seekMs={message.seekMs}
          playAfterSeek={message.playAfterSeek}
        />
      </div>
      <footer className="flex shrink-0 justify-between border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
        <span className="truncate" title={message.detail?.fileName ?? ""}>
          {message.detail?.fileName ?? "…"}
        </span>
        <span>Hold: original pixels · F: full screen · Escape: close</span>
      </footer>
    </div>
  );
}
