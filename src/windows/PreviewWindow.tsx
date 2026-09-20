import { useEffect, useRef, useState } from "react";
import { useI18n } from "../i18n/I18nContext";
import { listenThenAnnounce } from "../utils/handshake";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isComposingEvent } from "../hooks/useComposing";
import { isEditableTarget } from "../utils/shortcuts";
import { hasOpenModal } from "../utils/modalStack";
import { transcriptOwnsScrollKey } from "../utils/viewerKeys";
import PreviewSurface from "../components/PreviewSurface";
import type { PreviewPresentation, PreviewShowMessage } from "../state/preview-store";
import { message, type Message } from "../i18n/translate";
import { log, toErrorFields } from "../repositories";
import { recordActionFailure } from "../state/notifications-store";
import OperationResult from "../components/ui/OperationResult";
import {
  installBinariesEventWiring,
  useBinariesStore,
} from "../state/binaries-store";
import { installTranscriptEventWiring } from "../state/transcript-store";

// The separate Preview window renders the shared PreviewSurface
// from `preview://show` messages — payload AND detail arrive together from
// the anchor owner, so this window makes no library-selection query and can
// never race a stale response. It does load the small tool/config projections
// required by the shared media surface. The previous message keeps rendering
// until the next one arrives (no blank flash between keystrokes). Library
// commands go back to Main, which remains their one owner.

export default function PreviewWindow() {
  const { t, text } = useI18n();
  const [shown, setShown] = useState<PreviewShowMessage | null>(null);
  const [featuresReady, setFeaturesReady] = useState(false);
  const [actionError, setActionError] = useState<Message | null>(null);
  const [fullscreen, setFullscreen] = useState(false);
  const [presentationError, setPresentationError] = useState<Message | null>(null);
  const fullscreenRef = useRef(false);

  const reportActionError = (kind: string, failure: Message, error: unknown) => {
    log.error("preview window action failed", { kind, ...toErrorFields(error) });
    setActionError(failure);
    recordActionFailure(kind, failure, error);
  };

  useEffect(() => {
    let active = true;
    // Zustand stores are isolated per webview. Preview therefore admits the
    // two feature stores it renders instead of assuming Main's instances are
    // visible here.
    void Promise.all([
      installBinariesEventWiring(),
      installTranscriptEventWiring(),
    ]).then(async () => {
      await useBinariesStore.getState().load();
      if (active) setFeaturesReady(true);
    });

    // Ask the main window for the current selection — only once this window
    // can actually hear the reply (see handshake.ts).
    const unlisten = listenThenAnnounce<PreviewShowMessage>(
      "preview://show",
      "preview://ready",
      setShown,
    );
    const stopPresentation = listenThenAnnounce<PreviewPresentation>(
      "preview://fullscreen-state", "preview://ready", (value) => {
        fullscreenRef.current = value.fullscreen;
        setFullscreen(value.fullscreen);
        setPresentationError(value.error);
      },
    );
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || isComposingEvent(event) || hasOpenModal() || isEditableTarget(event.target)) return;
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      if (event.key === " ") {
        // Persistent Preview never reinterprets Space as playback or as a
        // second transient-viewer toggle.
        event.preventDefault();
        event.stopPropagation();
      } else if (event.key.toLowerCase() === "f") {
        event.preventDefault();
        event.stopPropagation();
        if (event.repeat) return;
        void emit("preview://fullscreen", "toggle")
          .then(() => setActionError(null))
          .catch((error) =>
            reportActionError(
              "preview-fullscreen-failed",
              message("preview.fullScreenFromPreviewFailed"),
              error,
            ),
          );
      } else if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        if (event.repeat) return;
        if (fullscreenRef.current) {
          void emit("preview://fullscreen", "exit").catch((error) =>
            reportActionError(
              "preview-fullscreen-failed",
              message("preview.leaveFullScreenFailed"),
              error,
            ));
          return;
        }
        void getCurrentWindow()
          .close()
          .catch((error) =>
            reportActionError(
              "preview-close-failed",
              message("preview.closeWindowFailed"),
              error,
            ),
          );
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
        if (transcriptOwnsScrollKey(event)) return;
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
          repeat: event.repeat,
          shiftKey: event.shiftKey,
          metaKey: event.metaKey,
          ctrlKey: event.ctrlKey,
          altKey: event.altKey,
        })
          .then(() => setActionError(null))
          .catch((error) =>
            reportActionError(
              "preview-command-failed",
              message("preview.commandFailed"),
              error,
            ),
          );
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      active = false;
      unlisten();
      stopPresentation();
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, []);

  if (shown === null || !featuresReady) {
    return (
      <div className="flex h-screen items-center justify-center bg-background">
        <p className="text-ink-muted">
          {t(shown === null ? "preview.selectItemInMain" : "preview.preparing")}
        </p>
      </div>
    );
  }

  const failure = actionError ?? presentationError;

  return (
    <div className="flex h-screen flex-col bg-background">
      <div className="min-h-0 flex-1">
        <PreviewSurface
          surface="preview-window"
          hash={shown.hash}
          detail={shown.detail}
          pathId={shown.pathId}
        />
      </div>
      {failure !== null ? (
        <OperationResult
          level="error"
          className="mx-3 mb-2 shrink-0"
          onDismiss={() => {
            setActionError(null);
            void emit("preview://dismiss-error").catch((error) =>
              reportActionError(
                "preview-dismiss-failed",
                message("preview.dismissFailed"),
                error,
              ));
          }}
          dismissLabel={t("preview.dismissResult")}
        >
          {text(failure)}
        </OperationResult>
      ) : null}
      <footer className="flex shrink-0 justify-between border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
        <span className="truncate" title={shown.detail?.fileName ?? ""}>
          {shown.detail?.fileName ?? "…"}
        </span>
        <div className="flex items-center gap-3">
          <span>
            {fullscreen ? t("preview.hintFullscreen") : t("preview.hint")}
          </span>
          <button className="rounded border border-border px-2 py-0.5 text-ink hover:bg-surface-muted"
            onClick={() => void emit("preview://fullscreen", "toggle").catch((error) =>
              reportActionError(
                "preview-fullscreen-failed",
                message("preview.fullScreenChangeFailed"),
                error,
              ))}>
            {fullscreen ? t("preview.leaveFullScreen") : t("preview.fullScreen")}
          </button>
        </div>
      </footer>
    </div>
  );
}
