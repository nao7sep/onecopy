import type { PendingDestinationConflicts } from "../models/destinationTransfer";
import { formatBytes } from "../models/items";
import { useDestinationsStore } from "../state/destinations-store";
import { resolveDestinationConflicts } from "../workflows/destinations";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";

function fileName(path: string): string {
  const trimmed = path.replace(/[/\\]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut < 0 ? trimmed : trimmed.slice(cut + 1);
}

export default function DestinationConflictModal({
  pending,
}: {
  pending: PendingDestinationConflicts;
}) {
  const close = () => useDestinationsStore.getState().setPendingConflicts(null);
  const operation = pending.mode === "copy" ? "Copy" : "Move";
  return (
    <ModalShell
      title={`${operation} name conflicts`}
      onClose={close}
      closeLabel="Cancel"
      widthClass="w-[min(760px,calc(100vw-3rem))]"
      primaryAction={
        <>
          <Button
            variant="primary"
            onClick={() => void resolveDestinationConflicts("rename")}
          >
            Rename and {operation.toLowerCase()}
          </Button>
          <Button
            variant="danger"
            disabled={!pending.overwriteAllowed}
            title={
              pending.overwriteAllowed
                ? "Preserve existing destination files in OneCopy Trash, then replace them"
                : "Overwrite cannot preserve two selected files that need the same destination name"
            }
            onClick={() => void resolveDestinationConflicts("overwrite")}
          >
            Overwrite
          </Button>
        </>
      }
    >
      <p className="text-sm text-ink">
        All selected files will use one choice. Nothing has been copied, moved,
        overwritten, or skipped yet.
      </p>
      <div className="mt-3 overflow-hidden border border-border">
        {pending.conflicts.map((conflict, index) => (
          <div
            key={`${conflict.path}-${index}`}
            className="border-b border-border px-3 py-2 last:border-b-0"
          >
            <p className="break-all text-sm font-medium text-ink-strong">
              {fileName(conflict.path)}
            </p>
            <p className="break-all text-xs text-ink-muted">{conflict.path}</p>
            <p className="mt-1 text-xs text-ink-muted">
              Incoming {formatBytes(conflict.incomingBytes)}
              {conflict.withinSelection
                ? " · More than one selected file needs this name"
                : conflict.existingBytes === null
                  ? " · Existing entry is not a replaceable regular file"
                  : ` · Existing ${formatBytes(conflict.existingBytes)}`}
            </p>
            {conflict.preservedPaths.length > 1 ? (
              <div className="mt-2 text-xs text-ink-muted">
                <p>Overwrite would preserve this companion family in Trash:</p>
                <ul className="mt-1 list-inside list-disc">
                  {conflict.preservedPaths.map((path) => (
                    <li key={path} className="break-all">
                      {path}
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}
          </div>
        ))}
      </div>
      {!pending.overwriteAllowed ? (
        <p className="mt-3 text-xs text-warning">
          Rename is required because overwriting would leave out part of the
          selected set or replace an entry that is not a regular file.
        </p>
      ) : (
        <p className="mt-3 text-xs text-ink-muted">
          Overwrite first preserves the existing destination files and their
          companion family in OneCopy Trash.
        </p>
      )}
    </ModalShell>
  );
}
