import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { formatBytes } from "../models/items";
import { log, toErrorFields } from "../repositories";
import ModalShell from "./ModalShell";
import ConfirmDialog from "./ConfirmDialog";
import Button from "./ui/Button";

// The Trash surface: every trash on the system — the configured volumes,
// the app home, AND any mounted drive carrying one from an earlier
// configuration — with per-root sizes, Reveal, and the one deliberately
// destructive convenience: Empty. Sizes are computed when the modal opens
// (it opens rarely; a cached number would only be a chance to lie).
// Emptying is PERMANENT — the trash is the safety net, and emptying removes
// the net for everything inside — so it confirms with the exact totals it
// is about to destroy. The trash stays write-only otherwise: the app NEVER
// purges on its own (by design — see the README's trash section).

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
    setRows(null);
    setError(null);
    void invoke<TrashRootInfo[]>("trash_overview")
      .then(setRows)
      .catch((error) => {
        log.error("trash overview failed", toErrorFields(error));
        setError("Trash locations are unavailable.");
      });
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
          ? "Trash was processed, but its totals couldn’t be refreshed."
          : `${outcomeError} Totals couldn’t be refreshed.`;
      }
      setError(outcomeError);
    } catch (error) {
      log.error("trash empty failed", toErrorFields(error));
      setError("Couldn’t empty this trash.");
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
    }
  };

  return (
    <ModalShell
      title="Trash"
      onClose={onClose}
      closeDisabled={busy}
      widthClass="w-[min(820px,calc(100vw-3rem))]"
      footerStart={rows !== null ? error : undefined}
    >
      {confirm !== null ? (
        <ConfirmDialog
          title="Empty this trash?"
          message={`Permanently delete ${confirm.files.toLocaleString()} file${
            confirm.files === 1 ? "" : "s"
          } (${formatBytes(confirm.bytes)}) from ${confirm.root}? The trash is the safety net — emptied files cannot be recovered.`}
          confirmLabel="Empty trash"
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
        Deleted files wait here — one trash per drive, so a delete is instant,
        and every attached drive is checked. The app never empties a trash on
        its own; emptying is permanent. Deleting these folders in the file
        manager is also always safe.
      </p>
      {rows === null ? (
        <p className={`py-4 text-center text-sm ${error !== null ? "text-danger" : "text-ink-muted"}`}>
          {error ?? "Measuring…"}
        </p>
      ) : rows.length === 0 ? (
        <p className="py-4 text-center text-sm text-ink-muted">No trash locations</p>
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
                  void revealItemInDir(row.root).catch((error) => {
                    log.warn("trash reveal failed", { root: row.root, ...toErrorFields(error) });
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
