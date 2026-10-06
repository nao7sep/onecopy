import { useEffect, useState } from "react";
import { useI18n } from "../i18n/I18nContext";
import { listenThenAnnounce } from "../utils/handshake";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isComposingEvent } from "../hooks/useComposing";
import { isEditableTarget } from "../utils/shortcuts";
import { hasOpenModal } from "../utils/modalStack";
import { controlOwnsForwardableKey } from "../utils/viewerKeys";
import PreviewSurface from "../components/PreviewSurface";
import type { PreviewShowMessage } from "../state/preview-store";
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
  const [publishedError, setPublishedError] = useState<Message | null>(null);

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
    const stopError = listenThenAnnounce<Message | null>(
      "preview://error", "preview://ready", setPublishedError,
    );
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || isComposingEvent(event) || hasOpenModal() || isEditableTarget(event.target)) return;
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      if (event.key === " ") {
        // Space does nothing here. The fullscreen view opens only from Main's
        // list, which holds the selection it shows; the preview window only
        // follows that selection, and Space is never its playback key.
        event.preventDefault();
        event.stopPropagation();
      } else if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        if (event.repeat) return;
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
        // Only a genuinely editable control consumes deletion keys; every
        // other forwarded key (arrows, paging, Enter) also stands down for a
        // native control that already owns it — a focused <audio controls>
        // or the transcript's own scroll region, exactly as the transient
        // viewer already decides through the shared predicate.
        const isDeletion = event.key === "Delete" || event.key === "Backspace";
        if (isDeletion ? isEditableTarget(event.target) : controlOwnsForwardableKey(event)) {
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
      stopError();
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

  const failure = actionError ?? publishedError;

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
      {/* The window shows only the anchor, so the footer names it and, with
          more than one selected, says how many: Delete here acts on all of
          them, as in Main. */}
      <footer className="flex shrink-0 justify-between border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
        <span className="flex min-w-0 items-center gap-2">
          <span className="truncate" title={shown.detail?.fileName ?? ""}>
            {shown.detail?.fileName ?? "…"}
          </span>
          {(shown.selectedCount ?? 0) > 1 ? (
            <span className="shrink-0">
              {t("preview.selectedCount", { count: shown.selectedCount ?? 0 })}
            </span>
          ) : null}
        </span>
        <span>{t("preview.hint")}</span>
      </footer>
    </div>
  );
}
