import { useEffect, useState } from "react";
import { listenThenAnnounce } from "../utils/handshake";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import PreviewSurface from "../components/PreviewSurface";
import type { PreviewShowMessage } from "../state/preview-store";
import { reportWindowCall } from "../repositories";

// The preview window placement (screen 2): renders the shared PreviewSurface
// from `preview://show` messages — payload AND detail arrive together from
// the anchor owner, so this window queries nothing and can never race a
// stale response. The previous message keeps rendering until the next one
// arrives (no blank flash between keystrokes). F toggles fullscreen; Escape
// leaves fullscreen, then closes (the main window observes the destroy and
// clears the follow flag).

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
      if (event.key.toLowerCase() === "f") {
        event.preventDefault();
        void (async () => {
          const window = getCurrentWindow();
          // Defaults to "not fullscreen" so a failed READ still lets F enter
          // and Escape close, rather than trapping the user in a surface
          // whose exit key does nothing — but the reason is logged, not lost.
          const full = await window.isFullscreen().catch((error: unknown) => {
            reportWindowCall("isFullscreen")(error);
            return false;
          });
          await window.setFullscreen(!full).catch(reportWindowCall("setFullscreen"));
        })();
      } else if (event.key === "Escape") {
        event.preventDefault();
        void (async () => {
          const window = getCurrentWindow();
          // Defaults to "not fullscreen" so a failed READ still lets F enter
          // and Escape close, rather than trapping the user in a surface
          // whose exit key does nothing — but the reason is logged, not lost.
          const full = await window.isFullscreen().catch((error: unknown) => {
            reportWindowCall("isFullscreen")(error);
            return false;
          });
          if (full) await window.setFullscreen(false).catch(reportWindowCall("setFullscreen"));
          else await window.close();
        })();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      unlisten();
      window.removeEventListener("keydown", onKeyDown);
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
        <span>Hold: original pixels · F: fullscreen · Escape: close</span>
      </footer>
    </div>
  );
}
