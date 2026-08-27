import { useEffect } from "react";
import { X } from "lucide-react";
import { useIssuesStore } from "../state/issues-store";
import { formatLocalMinute } from "../utils/displayTime";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";

// The Issues modal: current-state diagnostics, OLDEST first — the
// longest-standing condition leads. Scan-derived rows retire themselves when
// a scan finds the condition resolved; the Dismiss controls are the user's
// half, for the operation records nothing can re-check. Dismissing a
// condition that still exists is honest but temporary: the next scan that
// re-detects it brings it back. Retry is backend-authored and only appears
// for reconstructible output work; destructive intent is never replayed.

export default function IssuesModal() {
  const open = useIssuesStore((s) => s.open);
  const rows = useIssuesStore((s) => s.rows);
  const total = useIssuesStore((s) => s.total);
  const loading = useIssuesStore((s) => s.loading);
  const error = useIssuesStore((s) => s.error);
  const load = useIssuesStore((s) => s.load);
  const dismiss = useIssuesStore((s) => s.dismiss);
  const dismissAll = useIssuesStore((s) => s.dismissAll);
  const retry = useIssuesStore((s) => s.retry);
  const retryAll = useIssuesStore((s) => s.retryAll);
  const setOpen = useIssuesStore((s) => s.setOpen);

  useEffect(() => {
    if (open) void load();
  }, [open, load]);

  if (!open) return null;
  const retryable = rows.filter((row) => row.recovery?.status === "available").length;

  return (
    <ModalShell
      title="Issues"
      onClose={() => setOpen(false)}
      widthClass="w-[min(820px,calc(100vw-3rem))]"
      footerStart={
        error ??
        (total > rows.length ? `Showing the oldest ${rows.length} of ${total}` : undefined)
      }
      primaryAction={
        rows.length > 0 ? (
          <>
            {retryable > 0 ? (
              <Button onClick={() => void retryAll()}>Retry all</Button>
            ) : null}
            <Button variant="danger" onClick={() => void dismissAll()}>
              Dismiss all
            </Button>
          </>
        ) : undefined
      }
    >
      {rows.length === 0 ? (
        <p className={`py-6 text-center text-sm ${error !== null ? "text-danger" : "text-ink-muted"}`}>
          {loading ? "Loading issues…" : error ?? "No issues"}
        </p>
      ) : (
        <ul className="space-y-1.5">
          {rows.map((row) => (
            <li
              key={row.id}
              className="group rounded-lg border border-border bg-surface p-3 text-xs"
            >
              <div className="flex items-start justify-between gap-2">
                <span className="font-semibold text-danger">{row.kind}</span>
                <span className="flex shrink-0 items-center gap-2">
                  {row.recovery ? (
                    <Button
                      size="sm"
                      disabled={row.recovery.status === "queued"}
                      onClick={() => void retry(row.id)}
                    >
                      {row.recovery.status === "queued" ? "Queued" : row.recovery.label}
                    </Button>
                  ) : null}
                  <span className="text-ink-muted" title={`Last seen ${formatLocalMinute(row.lastSeenUtc)}`}>
                    {/* First-seen leads (the sort key); last-seen rides the
                        hover — two stamps in every row would be noise. */}
                    {formatLocalMinute(row.firstSeenUtc)}
                  </span>
                  <button
                    aria-label="Dismiss"
                    title="Dismiss"
                    className="flex h-5 w-5 items-center justify-center rounded text-ink-muted transition-colors hover:bg-danger-surface hover:text-danger"
                    onClick={() => void dismiss(row.id)}
                  >
                    <X size={12} />
                  </button>
                </span>
              </div>
              {row.path ? (
                <div className="mt-0.5 select-text break-all text-ink" title={row.path}>
                  {row.path}
                </div>
              ) : null}
              {row.message ? (
                <div className="mt-0.5 select-text break-words text-ink-muted">{row.message}</div>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </ModalShell>
  );
}
