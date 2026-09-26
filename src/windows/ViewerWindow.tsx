import { useEffect, useRef, useState } from "react";
import { useI18n } from "../i18n/I18nContext";
import { emit } from "@tauri-apps/api/event";
import { ChevronLeft, ChevronRight, Minimize2, X } from "lucide-react";
import { listenThenAnnounce } from "../utils/handshake";
import type { ViewerBroadcast } from "../workflows/quick-view";
import ConfirmModal from "../components/ConfirmModal";
import PreviewSurface from "../components/PreviewSurface";
import { message, type Message } from "../i18n/translate";
import { log, reportWindowCall, toErrorFields } from "../repositories";
import { viewerOwnsKey } from "../utils/viewerKeys";
import { hasOpenModal } from "../utils/modalStack";
import NotificationHost from "../components/NotificationHost";
import { recordActionFailure } from "../state/notifications-store";
import OperationResult from "../components/ui/OperationResult";

export default function ViewerWindow() {
  const { t, text } = useI18n();
  const [state, setState] = useState<ViewerBroadcast | null>(null);
  const [commandFailure, setCommandFailure] = useState<Message | null>(null);
  const surface = useRef<HTMLDivElement>(null);
  const pendingDeleteRef = useRef<ViewerBroadcast["pendingDelete"]>(null);
  const sectionKindRef = useRef<ViewerBroadcast["sectionKind"]>(null);
  const itemRef = useRef<ViewerBroadcast["item"]>(null);
  pendingDeleteRef.current = state?.pendingDelete ?? null;
  sectionKindRef.current = state?.sectionKind ?? null;
  itemRef.current = state?.item ?? null;
  const hasItem = state?.item != null;
  useEffect(() => {
    if (hasItem && !hasOpenModal()) surface.current?.focus();
  }, [hasItem]);

  const sendKey = (key: string, shiftKey = false): void => {
    void emit("viewer://key", { key, shiftKey })
      .then(() => setCommandFailure(null))
      .catch((error) => {
        log.error("viewer key forward failed", toErrorFields(error));
        const failure = message("viewer.commandFailed");
        setCommandFailure(failure);
        recordActionFailure("viewer-command-failed", failure, error);
      });
  };

  useEffect(() => {
    const unlisten = listenThenAnnounce<ViewerBroadcast>(
      "viewer://state",
      "viewer://ready",
      setState,
    );
    const onKeyDown = (event: KeyboardEvent) => {
      if (pendingDeleteRef.current !== null || hasOpenModal()
        || !viewerOwnsKey(event, sectionKindRef.current, itemRef.current?.fileName ?? "")) return;
      event.preventDefault();
      event.stopPropagation();
      void emit("viewer://key", {
        key: event.key,
        repeat: event.repeat,
        shiftKey: event.shiftKey,
        metaKey: event.metaKey,
        ctrlKey: event.ctrlKey,
        altKey: event.altKey,
      })
        .then(() => setCommandFailure(null))
        .catch((error) => {
          log.error("viewer key forward failed", toErrorFields(error));
          const failure = message("viewer.commandFailed");
          setCommandFailure(failure);
          recordActionFailure("viewer-command-failed", failure, error);
        });
    };
    const onFocus = () => {
      if (!hasOpenModal()) surface.current?.focus();
    };
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("focus", onFocus);
    return () => {
      unlisten();
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("focus", onFocus);
    };
  }, []);

  if (state?.item === null || state?.item === undefined) {
    return <div className="h-screen w-screen bg-black" />;
  }

  const item = state.item;
  const failure = commandFailure ?? state.failure;
  return (
    <div ref={surface} tabIndex={-1} aria-label={t("viewer.window")} className="relative flex h-screen w-screen flex-col overflow-hidden bg-black text-white outline-none">
      {/* Main, always open, is the one window that owns timed-notice
          auto-dismiss (Finding C) — this copy still shows and dismisses
          notices, it just never runs a second, unpausable countdown. */}
      <NotificationHost ownsTimedDismissal={false} />
      {/* Always visible, not hover-only: viewing-sessions.md's lightweight
          chrome shows filename, position, and prev/next as a standing
          contract, not a mouse-discoverable extra a keyboard-only or
          screen-reader user would never see (D9). */}
      <header className="absolute inset-x-0 top-0 z-10 flex items-center gap-2 bg-black/65 px-3 py-2 backdrop-blur-sm">
        <span className="min-w-0 flex-1 truncate text-sm" title={item.fileName}>
          {item.fileName}
        </span>
        <span className="text-xs tabular-nums text-white/70">
          {state.index + 1} / {state.length}
        </span>
        <button aria-label={t("viewer.previous")} disabled={state.index === 0} className="flex h-8 w-8 items-center justify-center rounded-md hover:bg-white/15 disabled:opacity-30" onClick={() => sendKey("ArrowLeft")}>
          <ChevronLeft size={16} />
        </button>
        <button aria-label={t("viewer.next")} disabled={state.index === state.length - 1} className="flex h-8 w-8 items-center justify-center rounded-md hover:bg-white/15 disabled:opacity-30" onClick={() => sendKey("ArrowRight")}>
          <ChevronRight size={16} />
        </button>
        <button aria-label={t("viewer.quickView")} className="flex h-8 w-8 items-center justify-center rounded-md hover:bg-white/15" onClick={() => sendKey(" ")}>
          <Minimize2 size={16} />
        </button>
        <button aria-label={t("viewer.closeFullScreen")} className="flex h-8 w-8 items-center justify-center rounded-md hover:bg-white/15" onClick={() => sendKey("Escape")}>
          <X size={16} />
        </button>
      </header>
      <div className="min-h-0 flex-1">
        {state.detail === null ? (
          <div className="flex h-full items-center justify-center text-sm text-white/60">{t("common.loading")}</div>
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
      {failure !== null ? (
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
                    const failure = message("viewer.dismissFailed");
                    setCommandFailure(failure);
                    recordActionFailure(
                      "viewer-result-dismiss-failed",
                      failure,
                      error,
                    );
                  });
                }
              }}
          dismissLabel={t("viewer.dismissResult")}
        >
          {text(failure)}
        </OperationResult>
      ) : null}
      {state.pendingDelete !== null ? (
        <ConfirmModal
          title={
            state.pendingDelete.kind === "permanent"
              ? t("common.deletePermanentlyTitle")
              : t("viewer.deleteTitle")
          }
          message={
            state.pendingDelete.kind === "permanent"
              ? t("viewer.deletePermanentlyBody", { name: state.pendingDelete.fileName })
              : t("viewer.deleteBody", { name: state.pendingDelete.fileName })
          }
          confirmLabel={
            state.pendingDelete.kind === "permanent"
              ? t("common.deletePermanently")
              : t("common.delete")
          }
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
