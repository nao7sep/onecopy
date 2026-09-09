import { useEffect } from "react";
import { X } from "lucide-react";
import { useIssuesStore } from "../state/issues-store";
import { formatLocalMinute } from "../utils/displayTime";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";

export default function IssuesModal({ open, onClose }: {
  open: boolean;
  onClose: () => void;
}) {
  const { rows, total, loading, error, load, dismiss, dismissAll } = useIssuesStore();
  useEffect(() => {
    if (open) void load();
  }, [open, load]);
  if (!open) return null;
  const footer = error ?? (total > rows.length
    ? `Showing the oldest ${rows.length} of ${total}` : undefined);

  return (
    <ModalShell
      title="Issues"
      onClose={onClose}
      widthClass="w-[min(820px,calc(100vw-3rem))]"
      footerStart={footer === undefined ? undefined : (
        <OperationResult level={error === null ? "info" : "error"}>{footer}</OperationResult>
      )}
      primaryAction={total > 0 ? (
        <Button variant="danger" onClick={() => void dismissAll()}>Dismiss all</Button>
      ) : undefined}
    >
      <p className="mb-3 text-sm text-ink-muted">
        Issues from this app session. Dismissed entries remain in diagnostic history.
      </p>
      {rows.length === 0 ? (
        error !== null ? (
          <OperationResult level="error" className="my-4">{error}</OperationResult>
        ) : (
          <p className="py-6 text-center text-sm text-ink-muted">
            {loading ? "Loading issues…" : "No issues"}
          </p>
        )
      ) : (
        <ul className="space-y-1.5">
          {rows.map((row) => (
            <li key={row.id} className="rounded-lg border border-border bg-surface p-3 text-xs">
              <div className="flex items-start justify-between gap-3">
                <span className="flex min-w-0 flex-wrap items-center gap-2 text-ink-muted">
                  <span title={`Last seen ${formatLocalMinute(row.lastSeenUtc)}`}>
                    {formatLocalMinute(row.firstSeenUtc)}
                  </span>
                  {row.occurrenceCount > 1 ? <span>×{row.occurrenceCount}</span> : null}
                </span>
                <button
                  aria-label="Dismiss"
                  title="Dismiss"
                  className="flex h-5 w-5 shrink-0 items-center justify-center rounded text-ink-muted transition-colors hover:bg-danger-surface hover:text-danger"
                  onClick={() => void dismiss(row.id)}
                ><X size={12} /></button>
              </div>
              {row.path ? <div className="mt-2 select-text break-all text-ink" title={row.path}>{row.path}</div> : null}
              {row.message ? <div className="mt-1.5 select-text break-words leading-relaxed text-ink-muted">{row.message}</div> : null}
            </li>
          ))}
        </ul>
      )}
    </ModalShell>
  );
}
