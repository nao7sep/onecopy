import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { formatBytes } from "../models/items";
import { log, toErrorFields } from "../repositories";
import ModalShell from "./ModalShell";
import ConfirmDialog from "./ConfirmDialog";
import Button from "./ui/Button";

// The Trash surface: per-root sizes and the one deliberately destructive
// convenience — Empty. Sizes are computed when the modal opens (it opens
// rarely; a cached number would only be a chance to lie). Emptying is
// PERMANENT — the trash is the safety net, and emptying removes the net for
// everything inside — so it confirms with the exact totals it is about to
// destroy. The trash stays write-only otherwise.

interface TrashRootInfo {
  root: string;
  bytes: number;
  files: number;
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

  useEffect(() => {
    if (!open) return;
    setRows(null);
    void invoke<TrashRootInfo[]>("trash_overview")
      .then(setRows)
      .catch((error) => {
        log.error("trash overview failed", toErrorFields(error));
        setRows([]);
      });
  }, [open]);

  if (!open) return null;

  const empty = async (row: TrashRootInfo) => {
    setBusy(true);
    try {
      await invoke("trash_empty", { root: row.root });
      setRows(await invoke<TrashRootInfo[]>("trash_overview"));
    } catch (error) {
      log.error("trash empty failed", toErrorFields(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <ModalShell title="Trash" onClose={onClose} widthClass="w-[560px]">
      {confirm !== null ? (
        <ConfirmDialog
          title="Empty this trash?"
          message={`Permanently delete ${confirm.files.toLocaleString()} file${
            confirm.files === 1 ? "" : "s"
          } (${formatBytes(confirm.bytes)}) from ${confirm.root}? The trash is the safety net — emptied files cannot be recovered.`}
          confirmLabel="Empty trash"
          onConfirm={() => {
            const row = confirm;
            setConfirm(null);
            void empty(row);
          }}
          onCancel={() => setConfirm(null)}
        />
      ) : null}
      <p className="mb-3 text-sm text-ink-muted">
        Deleted files wait here — one trash per drive, so a delete is instant.
        Emptying is permanent. Deleting these folders in the file manager is
        also always safe.
      </p>
      {rows === null ? (
        <p className="py-4 text-center text-sm text-ink-muted">Measuring…</p>
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
              </div>
              <Button
                variant="danger"
                disabled={busy || row.files === 0}
                onClick={() => setConfirm(row)}
              >
                Empty
              </Button>
            </li>
          ))}
        </ul>
      )}
    </ModalShell>
  );
}
