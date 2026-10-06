import { useEffect, useRef, useState } from "react";
import { useI18n } from "../i18n/I18nContext";
import { emit } from "@tauri-apps/api/event";
import { ChevronLeft, ChevronRight, X } from "lucide-react";
import { listenThenAnnounce } from "../utils/handshake";
import type { FullscreenViewBroadcast } from "../workflows/fullscreen-view";
import ConfirmModal from "../components/ConfirmModal";
import PreviewSurface from "../components/PreviewSurface";
import { message, type Message } from "../i18n/translate";
import { log, reportWindowCall, toErrorFields } from "../repositories";
import { fullscreenViewOwnsKey } from "../utils/viewerKeys";
import { hasOpenModal } from "../utils/modalStack";
import NotificationHost from "../components/NotificationHost";
import { recordActionFailure } from "../state/notifications-store";
import OperationResult from "../components/ui/OperationResult";

export default function FullscreenViewWindow() {
  const { t, text } = useI18n();
  const [state, setState] = useState<FullscreenViewBroadcast | null>(null);
  const [commandFailure, setCommandFailure] = useState<Message | null>(null);
  const surface = useRef<HTMLDivElement>(null);
  const pendingDeleteRef = useRef<FullscreenViewBroadcast["pendingDelete"]>(null);
  const sectionKindRef = useRef<FullscreenViewBroadcast["sectionKind"]>(null);
  const itemRef = useRef<FullscreenViewBroadcast["item"]>(null);
  pendingDeleteRef.current = state?.pendingDelete ?? null;
  sectionKindRef.current = state?.sectionKind ?? null;
  itemRef.current = state?.item ?? null;
  const hasItem = state?.item != null;
  useEffect(() => {
    if (hasItem && !hasOpenModal()) surface.current?.focus();
  }, [hasItem]);

  const sendKey = (key: string, shiftKey = false): void => {
    void emit("fullscreen-view://key", { key, shiftKey })
      .then(() => setCommandFailure(null))
      .catch((error) => {
        log.error("fullscreen view key forward failed", toErrorFields(error));
        const failure = message("fullscreenView.commandFailed");
        setCommandFailure(failure);
        recordActionFailure("fullscreen-view-command-failed", failure, error);
      });
  };

  useEffect(() => {
    const unlisten = listenThenAnnounce<FullscreenViewBroadcast>(
      "fullscreen-view://state",
      "fullscreen-view://ready",
      setState,
    );
    const onKeyDown = (event: KeyboardEvent) => {
      if (pendingDeleteRef.current !== null || hasOpenModal()
        || !fullscreenViewOwnsKey(event, sectionKindRef.current, itemRef.current?.fileName ?? "")) return;
      event.preventDefault();
      event.stopPropagation();
      void emit("fullscreen-view://key", {
        key: event.key,
        repeat: event.repeat,
        shiftKey: event.shiftKey,
        metaKey: event.metaKey,
        ctrlKey: event.ctrlKey,
        altKey: event.altKey,
      })
        .then(() => setCommandFailure(null))
        .catch((error) => {
          log.error("fullscreen view key forward failed", toErrorFields(error));
          const failure = message("fullscreenView.commandFailed");
          setCommandFailure(failure);
          recordActionFailure("fullscreen-view-command-failed", failure, error);
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
    <div ref={surface} tabIndex={-1} aria-label={t("fullscreenView.window")} className="group relative flex h-screen w-screen flex-col overflow-hidden bg-black text-white outline-none">
      {/* Main, always open, is the one window that owns timed-notice
          auto-dismiss (Finding C) — this copy still shows and dismisses
          notices, it just never runs a second, unpausable countdown. */}
      <NotificationHost ownsTimedDismissal={false} />
      {/* Hover- and focus-reveal (D9, reverted): the lightweight chrome
          stays reachable by keyboard (Tab reaches it, focus-within keeps it
          shown) without permanently overlaying the media beneath it. */}
      <header className="absolute inset-x-0 top-0 z-10 flex items-center gap-2 bg-black/65 px-3 py-2 opacity-0 backdrop-blur-sm transition-opacity group-hover:opacity-100 focus-within:opacity-100">
        <span className="min-w-0 flex-1 truncate text-sm" title={item.fileName}>
          {item.fileName}
        </span>
        <span className="text-xs tabular-nums text-white/70">
          {state.index + 1} / {state.length}
        </span>
        <button aria-label={t("fullscreenView.previous")} disabled={state.index === 0} className="flex h-8 w-8 items-center justify-center rounded-md hover:bg-white/15 disabled:opacity-30" onClick={() => sendKey("ArrowLeft")}>
          <ChevronLeft size={16} />
        </button>
        <button aria-label={t("fullscreenView.next")} disabled={state.index === state.length - 1} className="flex h-8 w-8 items-center justify-center rounded-md hover:bg-white/15 disabled:opacity-30" onClick={() => sendKey("ArrowRight")}>
          <ChevronRight size={16} />
        </button>
        <button aria-label={t("fullscreenView.close")} className="flex h-8 w-8 items-center justify-center rounded-md hover:bg-white/15" onClick={() => sendKey("Escape")}>
          <X size={16} />
        </button>
      </header>
      <div className="min-h-0 flex-1">
        {state.detail === null ? (
          <div className="flex h-full items-center justify-center text-sm text-white/60">{t("common.loading")}</div>
        ) : (
          <PreviewSurface
            surface="fullscreen-view"
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
                  void emit("fullscreen-view://dismiss-failure", {}).catch((error) => {
                    log.error(
                      "fullscreen view failure dismissal failed",
                      toErrorFields(error),
                    );
                    const failure = message("fullscreenView.dismissFailed");
                    setCommandFailure(failure);
                    recordActionFailure(
                      "fullscreen-view-result-dismiss-failed",
                      failure,
                      error,
                    );
                  });
                }
              }}
          dismissLabel={t("fullscreenView.dismissResult")}
        >
          {text(failure)}
        </OperationResult>
      ) : null}
      {state.pendingDelete !== null ? (
        <ConfirmModal
          title={
            state.pendingDelete.kind === "permanent"
              ? t("common.deletePermanentlyTitle")
              : t("fullscreenView.deleteTitle")
          }
          message={
            state.pendingDelete.kind === "permanent"
              ? t("fullscreenView.deletePermanentlyBody", { name: state.pendingDelete.fileName })
              : t("fullscreenView.deleteBody", { name: state.pendingDelete.fileName })
          }
          confirmLabel={
            state.pendingDelete.kind === "permanent"
              ? t("common.deletePermanently")
              : t("common.delete")
          }
          onConfirm={() => {
            void emit("fullscreen-view://confirm-delete", {}).catch(
              reportWindowCall("fullscreen view delete confirmation"),
            );
          }}
          onCancel={() => {
            void emit("fullscreen-view://cancel-delete", {}).catch(
              reportWindowCall("fullscreen view delete cancellation"),
            );
          }}
        />
      ) : null}
    </div>
  );
}
