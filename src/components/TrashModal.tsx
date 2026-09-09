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
  const [rows, setRows] = useState<TrashRootInfo[] | null>(null);
  const [confirm, setConfirm] = useState<TrashRootInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [activeRoot, setActiveRoot] = useState<string | null>(null);
  const [progress, setProgress] = useState<TrashEmptyProgress | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [error, setError] = useState<string | null>(null);

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
        setError("Deleted-file locations are unavailable.");
        recordActionFailure("trash-overview-failed", "Deleted-file locations are unavailable.", error);
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
      if (alive) setError("Live deletion progress is unavailable.");
      recordActionFailure("trash-progress-unavailable", "Live deletion progress is unavailable.", error);
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
      let outcomeError = outcome.failures > 0
        ? `${outcome.failures.toLocaleString()} entr${outcome.failures === 1 ? "y" : "ies"} could not be removed.`
        : null;
      try {
        setRows(await invoke<TrashRootInfo[]>("trash_overview"));
      } catch (error) {
        log.error("trash remeasurement failed", toErrorFields(error));
        setRows(null);
        outcomeError = outcomeError === null
          ? "Deleted files were processed, but their totals couldn’t be refreshed."
          : `${outcomeError} Totals couldn’t be refreshed.`;
      }
      setError(outcomeError);
      if (outcomeError !== null) {
        recordActionFailure("trash-empty-partial", outcomeError);
      }
    } catch (error) {
      log.error("trash empty failed", toErrorFields(error));
      setError("Couldn’t empty these deleted files.");
      recordActionFailure("trash-empty-failed", "Couldn’t empty these deleted files.", error);
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
      setError("Couldn’t cancel emptying deleted files.");
      recordActionFailure("trash-empty-cancel-failed", "Couldn’t cancel emptying deleted files.", error);
    }
  };

  return (
    <ModalShell
      title="Deleted files"
      onClose={onClose}
      closeDisabled={busy}
      widthClass="w-[min(820px,calc(100vw-3rem))]"
      footerResult={
        rows !== null && error !== null ? (
          <OperationResult level="error">{error}</OperationResult>
        ) : undefined
      }
    >
      {confirm !== null ? (
        <ConfirmDialog
          title="Empty deleted files?"
          message={`Permanently delete ${confirm.files.toLocaleString()} file${
            confirm.files === 1 ? "" : "s"
          } (${formatBytes(confirm.bytes)}) from ${confirm.root}? Emptied files cannot be recovered.`}
          confirmLabel="Empty deleted files"
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
        Each configured root keeps its deleted files beneath its own permission
        boundary. OneCopy never empties these folders automatically. Emptying
        is permanent; removing their contents in the file manager is also safe.
      </p>
      {rows === null ? (
        error !== null ? (
          <OperationResult level="error" className="my-4">
            {error}
          </OperationResult>
        ) : (
          <p className="py-4 text-center text-sm text-ink-muted">Measuring…</p>
        )
      ) : rows.length === 0 ? (
        <p className="py-4 text-center text-sm text-ink-muted">No deleted-file locations</p>
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
                  {row.files.toLocaleString()} file{row.files === 1 ? "" : "s"} ·{" "}
                  {formatBytes(row.bytes)}
                </p>
                {activeRoot === row.root ? (
                  <p className="mt-1 text-xs tabular-nums text-primary">
                    {cancelling
                      ? "Cancelling…"
                      : progress === null
                        ? "Starting…"
                        : `Removing — ${progress.done.toLocaleString()}/${progress.total.toLocaleString()} · ${formatBytes(progress.bytesDone)}/${formatBytes(progress.bytesTotal)}${progress.failures > 0 ? ` · ${progress.failures.toLocaleString()} failed` : ""}`}
                  </p>
                ) : null}
              </div>
              <Button
                onClick={() => {
                  void invoke("trash_reveal", { root: row.root }).catch((error) => {
                    log.warn("trash reveal failed", { root: row.root, ...toErrorFields(error) });
                    setError("Couldn’t reveal this deleted-files location.");
                  });
                }}
              >
                Reveal
              </Button>
              {activeRoot === row.root ? (
                <Button disabled={cancelling} onClick={() => void cancelEmpty()}>
                  {cancelling ? "Cancelling…" : "Cancel"}
                </Button>
              ) : (
                <Button
                  variant="danger"
                  disabled={busy || row.files === 0}
                  onClick={() => setConfirm(row)}
                >
                  Empty
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
    </ModalShell>
  );
}
