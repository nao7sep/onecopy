import { useEffect, useRef, useState } from "react";
import { emit } from "@tauri-apps/api/event";
import { ChevronLeft, ChevronRight, Minimize2, X } from "lucide-react";
import { listenThenAnnounce } from "../utils/handshake";
import type { ViewerBroadcast } from "../workflows/quick-view";
import ConfirmDialog from "../components/ConfirmDialog";
import PreviewSurface from "../components/PreviewSurface";
import { log, reportWindowCall, toErrorFields } from "../repositories";
import { isAudioFile } from "../models/items";
import NotificationHost from "../components/NotificationHost";
import { recordActionFailure } from "../state/notifications-store";
import OperationResult from "../components/ui/OperationResult";

export default function ViewerWindow() {
  const [state, setState] = useState<ViewerBroadcast | null>(null);
  const [commandFailure, setCommandFailure] = useState<string | null>(null);
  const pendingDeleteRef = useRef<ViewerBroadcast["pendingDelete"]>(null);
  const sectionKindRef = useRef<ViewerBroadcast["sectionKind"]>(null);
  const itemRef = useRef<ViewerBroadcast["item"]>(null);
  pendingDeleteRef.current = state?.pendingDelete ?? null;
  sectionKindRef.current = state?.sectionKind ?? null;
  itemRef.current = state?.item ?? null;

  const sendKey = (key: string, shiftKey = false): void => {
    void emit("viewer://key", { key, shiftKey })
      .then(() => setCommandFailure(null))
      .catch((error) => {
        log.error("viewer key forward failed", toErrorFields(error));
        const message = "Couldn’t send this viewer command.";
        setCommandFailure(message);
        recordActionFailure("viewer-command-failed", message, error);
      });
  };

  useEffect(() => {
    const unlisten = listenThenAnnounce<ViewerBroadcast>(
      "viewer://state",
      "viewer://ready",
      setState,
    );
    const onKeyDown = (event: KeyboardEvent) => {
      const notificationControl =
        event.target instanceof Element && event.target.closest("[data-notification]") !== null;
      if (notificationControl && event.key === "Enter") return;
      const handled = [
          "Escape",
          " ",
          "f",
          "F",
          "ArrowLeft",
          "ArrowRight",
          "Delete",
          "Backspace",
        ].includes(event.key) ||
        (sectionKindRef.current !== "other" && ["PageUp", "PageDown", "Home", "End"].includes(event.key)) ||
        (event.key === "Enter" &&
          (sectionKindRef.current === "video" ||
            (itemRef.current !== null && isAudioFile(itemRef.current.fileName))));
      if (!handled) return;
      if (pendingDeleteRef.current !== null) return;
      event.preventDefault();
      event.stopPropagation();
      void emit("viewer://key", {
        key: event.key,
        shiftKey: event.shiftKey,
        metaKey: event.metaKey,
        ctrlKey: event.ctrlKey,
        altKey: event.altKey,
      })
        .then(() => setCommandFailure(null))
        .catch((error) => {
          log.error("viewer key forward failed", toErrorFields(error));
          const message = "Couldn’t send this viewer command.";
          setCommandFailure(message);
          recordActionFailure("viewer-command-failed", message, error);
        });
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      unlisten();
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, []);

  if (state?.item === null || state?.item === undefined) {
    return <div className="h-screen w-screen bg-black" />;
  }

  const item = state.item;
  return (
    <div className="group relative flex h-screen w-screen flex-col overflow-hidden bg-black text-white">
      <NotificationHost />
      <header className="absolute inset-x-0 top-0 z-10 flex items-center gap-2 bg-black/65 px-3 py-2 opacity-0 backdrop-blur-sm transition-opacity group-hover:opacity-100 focus-within:opacity-100">
        <span className="min-w-0 flex-1 truncate text-sm" title={item.fileName}>
          {item.fileName}
        </span>
        <span className="text-xs tabular-nums text-white/70">
          {state.index + 1} / {state.length}
        </span>
        <button aria-label="Previous item" disabled={state.index === 0} className="flex h-8 w-8 items-center justify-center rounded-md hover:bg-white/15 disabled:opacity-30" onClick={() => sendKey("ArrowLeft")}>
          <ChevronLeft size={16} />
        </button>
        <button aria-label="Next item" disabled={state.index === state.length - 1} className="flex h-8 w-8 items-center justify-center rounded-md hover:bg-white/15 disabled:opacity-30" onClick={() => sendKey("ArrowRight")}>
          <ChevronRight size={16} />
        </button>
        <button aria-label="Switch to Quick View" className="flex h-8 w-8 items-center justify-center rounded-md hover:bg-white/15" onClick={() => sendKey(" ")}>
          <Minimize2 size={16} />
        </button>
        <button aria-label="Close full screen" className="flex h-8 w-8 items-center justify-center rounded-md hover:bg-white/15" onClick={() => sendKey("Escape")}>
          <X size={16} />
        </button>
      </header>
      <div className="min-h-0 flex-1">
        {state.detail === null ? (
          <div className="flex h-full items-center justify-center text-sm text-white/60">Loading…</div>
        ) : (
          <PreviewSurface
            surface="viewer"
            hash={item.hash}
            pathId={item.hash === null ? item.pathId : null}
            detail={state.detail}
            keyboardActive
          />
        )}
      </div>
      {state.failure !== null || commandFailure !== null ? (
        <OperationResult
          level="error"
          className="absolute bottom-4 left-1/2 z-20 w-[min(520px,calc(100vw-2rem))] -translate-x-1/2 shadow-xl"
          onDismiss={() => {
                setCommandFailure(null);
                if (state.failure !== null) {
                  void emit("viewer://dismiss-failure", {}).catch((error) => {
                    log.error(
                      "viewer failure dismissal failed",
                      toErrorFields(error),
                    );
                    const message = "Couldn’t dismiss this viewer result.";
                    setCommandFailure(message);
                    recordActionFailure(
                      "viewer-result-dismiss-failed",
                      message,
                      error,
                    );
                  });
                }
              }}
          dismissLabel="Dismiss viewer result"
        >
          {commandFailure ?? state.failure}
        </OperationResult>
      ) : null}
      {state.pendingDelete !== null ? (
        <ConfirmDialog
          title={state.pendingDelete === "permanent" ? "Delete permanently?" : "Move to trash?"}
          message={`${state.pendingDelete === "permanent" ? "Permanently delete" : "Move"} ${item.fileName}${
            state.pendingDelete === "permanent"
              ? " and every copy? This cannot be undone."
              : " and every copy to the trash?"
          }`}
          confirmLabel={state.pendingDelete === "permanent" ? "Delete permanently" : "Move to trash"}
          onConfirm={() => {
            void emit("viewer://confirm-delete", {}).catch(
              reportWindowCall("viewer delete confirmation"),
            );
          }}
          onCancel={() => {
            void emit("viewer://cancel-delete", {}).catch(
              reportWindowCall("viewer delete cancellation"),
            );
          }}
        />
      ) : null}
    </div>
  );
}
