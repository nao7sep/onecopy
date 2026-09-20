import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { formatBytes } from "../models/items";
import { log, toErrorFields } from "../repositories";
import ModalShell from "./ModalShell";
import ConfirmDialog from "./ConfirmDialog";
import Button from "./ui/Button";
import { recordActionFailure } from "../state/notifications-store";
import OperationResult from "./ui/OperationResult";
import { useI18n } from "../i18n/I18nContext";
import { message, type Message } from "../i18n/translate";

// Deleted files: one permission-preserving location below each configured
// source or destination root, with per-root sizes, Reveal, and the deliberately
// destructive convenience: Empty. Sizes are computed when the modal opens
// (it opens rarely; a cached number would only be a chance to lie).
// Emptying is PERMANENT, so it confirms with the exact totals it is about to
// destroy. The storage stays write-only otherwise: the app never purges it.

interface TrashRootInfo {
  root: string;
  bytes: number;
  files: number;
}

interface TrashEmptyProgress {
  done: number;
  total: number;
  bytesDone: number;
  bytesTotal: number;
  failures: number;
}

interface TrashEmptyOutcome {
  cancelled: boolean;
  failures: number;
}

export default function TrashModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const { t, text, number } = useI18n();
  const [rows, setRows] = useState<TrashRootInfo[] | null>(null);
  const [confirm, setConfirm] = useState<TrashRootInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [activeRoot, setActiveRoot] = useState<string | null>(null);
  const [progress, setProgress] = useState<TrashEmptyProgress | null>(null);
  const [cancelling, setCancelling] = useState(false);
  // The message, not a finished sentence, so it follows a language change.
  const [error, setError] = useState<Message | null>(null);

  useEffect(() => {
    if (!open) return;
    let current = true;
    setRows(null);
    setError(null);
    void invoke<TrashRootInfo[]>("trash_overview")
      .then((result) => {
        if (current) setRows(result);
      })
      .catch((error) => {
        if (!current) return;
        log.error("trash overview failed", toErrorFields(error));
        setError(message("trash.locationsUnavailable"));
        recordActionFailure(
          "trash-overview-failed",
          message("trash.locationsUnavailable"),
          error,
        );
      });
    return () => {
      current = false;
    };
  }, [open]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let alive = true;
    void listen<{ root: string; progress: TrashEmptyProgress }>(
      "trash://progress",
      (event) => {
        if (!alive || !busyRef.current) return;
        setActiveRoot(event.payload.root);
        setProgress(event.payload.progress);
      },
    ).then((stop) => {
      if (alive) unlisten = stop;
      else stop();
    }).catch((error) => {
      log.warn("trash progress wiring failed", toErrorFields(error));
      if (alive) setError(message("trash.progressUnavailable"));
      recordActionFailure(
        "trash-progress-unavailable",
        message("trash.progressUnavailable"),
        error,
      );
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  if (!open) return null;

  const empty = async (row: TrashRootInfo) => {
    busyRef.current = true;
    setBusy(true);
    setActiveRoot(row.root);
    setProgress(null);
    setCancelling(false);
    setError(null);
    try {
      const outcome = await invoke<TrashEmptyOutcome>("trash_empty", { root: row.root });
      let outcomeError: Message | null = outcome.failures > 0
        ? message("trash.entriesNotRemoved", { count: outcome.failures })
        : null;
      try {
        setRows(await invoke<TrashRootInfo[]>("trash_overview"));
      } catch (error) {
        log.error("trash remeasurement failed", toErrorFields(error));
        setRows(null);
        outcomeError = outcomeError === null
          ? message("trash.totalsNotRefreshed")
          : message("trash.partialTotalsNotRefreshed", { reason: outcomeError });
      }
      setError(outcomeError);
      if (outcomeError !== null) {
        recordActionFailure("trash-empty-partial", outcomeError);
      }
    } catch (error) {
      log.error("trash empty failed", toErrorFields(error));
      setError(message("trash.emptyFailed"));
      recordActionFailure("trash-empty-failed", message("trash.emptyFailed"), error);
    } finally {
      busyRef.current = false;
      setBusy(false);
      setActiveRoot(null);
      setProgress(null);
      setCancelling(false);
    }
  };

  const cancelEmpty = async () => {
    if (!busy || cancelling) return;
    setCancelling(true);
    try {
      const active = await invoke<boolean>("trash_empty_cancel");
      if (!active) setCancelling(false);
    } catch (error) {
      setCancelling(false);
      log.error("trash empty cancellation failed", toErrorFields(error));
      setError(message("trash.cancelFailed"));
      recordActionFailure(
        "trash-empty-cancel-failed",
        message("trash.cancelFailed"),
        error,
      );
    }
  };

  return (
    <ModalShell
      title={t("trash.title")}
      onClose={onClose}
      closeDisabled={busy}
      widthClass="w-[min(820px,calc(100vw-3rem))]"
      footerResult={
        rows !== null && error !== null ? (
          <OperationResult level="error">{text(error)}</OperationResult>
        ) : undefined
      }
    >
      {confirm !== null ? (
        <ConfirmDialog
          title={t("trash.confirmTitle")}
          message={t("trash.confirmMessage", {
            count: confirm.files,
            size: formatBytes(confirm.bytes, number),
            root: confirm.root,
          })}
          confirmLabel={t("trash.confirmAction")}
          widthClass="w-[min(820px,calc(100vw-3rem))]"
          onConfirm={() => {
            const row = confirm;
            setConfirm(null);
            void empty(row);
          }}
          onCancel={() => setConfirm(null)}
        />
      ) : null}
      <p className="mb-3 text-sm text-ink-muted">
        {t("trash.intro")}
      </p>
      {rows === null ? (
        error !== null ? (
          <OperationResult level="error" className="my-4">
            {text(error)}
          </OperationResult>
        ) : (
          <p className="py-4 text-center text-sm text-ink-muted">{t("trash.measuring")}</p>
        )
      ) : rows.length === 0 ? (
        <p className="py-4 text-center text-sm text-ink-muted">{t("trash.noLocations")}</p>
      ) : (
        <ul className="space-y-1.5">
          {rows.map((row) => (
            <li
              key={row.root}
              className="flex items-center gap-3 rounded-lg border border-border px-3 py-2"
            >
              <div className="min-w-0 flex-1">
                <p className="select-text break-all text-sm text-ink">{row.root}</p>
                <p className="text-xs tabular-nums text-ink-muted">
                  {t("trash.rootSummary", {
                    count: row.files,
                    size: formatBytes(row.bytes, number),
                  })}
                </p>
                {activeRoot === row.root ? (
                  <p className="mt-1 text-xs tabular-nums text-primary">
                    {cancelling
                      ? t("common.cancelling")
                      : progress === null
                        ? t("common.starting")
                        : [
                            t("trash.removingProgress", {
                              done: progress.done,
                              total: progress.total,
                              doneSize: formatBytes(progress.bytesDone, number),
                              totalSize: formatBytes(progress.bytesTotal, number),
                            }),
                            progress.failures > 0
                              ? t("trash.failedCount", { count: progress.failures })
                              : null,
                          ].filter((part) => part !== null).join(" · ")}
                  </p>
                ) : null}
              </div>
              <Button
                onClick={() => {
                  void invoke("trash_reveal", { root: row.root }).catch((error) => {
                    log.warn("trash reveal failed", { root: row.root, ...toErrorFields(error) });
                    setError(message("trash.revealFailed"));
                  });
                }}
              >
                {t("trash.reveal")}
              </Button>
              {activeRoot === row.root ? (
                <Button disabled={cancelling} onClick={() => void cancelEmpty()}>
                  {cancelling ? t("common.cancelling") : t("common.cancel")}
                </Button>
              ) : (
                <Button
                  variant="danger"
                  disabled={busy || row.files === 0}
                  onClick={() => setConfirm(row)}
                >
                  {t("trash.empty")}
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
    </ModalShell>
  );
}
