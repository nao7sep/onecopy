import { useEffect } from "react";
import { X } from "lucide-react";
import { useIssuesStore } from "../state/issues-store";
import { formatLocalMinute } from "../utils/displayTime";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";

// Active is current-state diagnosis, oldest first; Recent is bounded history,
// newest first. Scan-derived Active rows retire when resolved, user dismissal
// may be temporary when a later check rediscovers the condition, and only
// backend-authored safe recovery is offered. Destructive intent is never
// replayed.

export default function IssuesModal() {
  const open = useIssuesStore((s) => s.open);
  const rows = useIssuesStore((s) => s.rows);
  const total = useIssuesStore((s) => s.total);
  const loading = useIssuesStore((s) => s.loading);
  const error = useIssuesStore((s) => s.error);
  const recentRows = useIssuesStore((s) => s.recentRows);
  const recentTotal = useIssuesStore((s) => s.recentTotal);
  const recentLoading = useIssuesStore((s) => s.recentLoading);
  const recentError = useIssuesStore((s) => s.recentError);
  const view = useIssuesStore((s) => s.view);
  const load = useIssuesStore((s) => s.load);
  const dismiss = useIssuesStore((s) => s.dismiss);
  const dismissAll = useIssuesStore((s) => s.dismissAll);
  const recover = useIssuesStore((s) => s.recover);
  const retryAll = useIssuesStore((s) => s.retryAll);
  const setOpen = useIssuesStore((s) => s.setOpen);
  const setView = useIssuesStore((s) => s.setView);

  useEffect(() => {
    if (open) void load();
  }, [open, load]);

  if (!open) return null;
  const retryable = rows.filter(
    (row) => row.recovery?.action === "retry" && row.recovery.status === "available",
  ).length;
  const activeView = view === "active";
  const footerError = activeView
    ? error ?? (total > rows.length ? `Showing the oldest ${rows.length} of ${total}` : undefined)
    : recentError ??
      (recentTotal > recentRows.length
        ? `Showing the newest ${recentRows.length} of ${recentTotal}`
        : undefined);
  const footerLevel = activeView
    ? error === null
      ? "info"
      : "error"
    : recentError === null
      ? "info"
      : "error";

  return (
    <ModalShell
      title="Issues"
      onClose={() => setOpen(false)}
      widthClass="w-[min(820px,calc(100vw-3rem))]"
      footerStart={
        footerError === undefined ? undefined : (
          <OperationResult level={footerLevel}>{footerError}</OperationResult>
        )
      }
      primaryAction={
        activeView && rows.length > 0 ? (
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
      <div className="mb-3 flex border-b border-border" role="tablist" aria-label="Issue history">
        {(["active", "recent"] as const).map((tab, index, tabs) => (
          <button
            key={tab}
            id={`issues-tab-${tab}`}
            role="tab"
            aria-selected={view === tab}
            aria-controls={`issues-panel-${tab}`}
            tabIndex={view === tab ? 0 : -1}
            className={`border-b-2 px-3 py-2 text-sm ${
              view === tab
                ? "border-primary font-semibold text-ink-strong"
                : "border-transparent text-ink-muted hover:text-ink"
            }`}
            onClick={() => setView(tab)}
            onKeyDown={(event) => {
              const target =
                event.key === "ArrowRight"
                  ? Math.min(index + 1, tabs.length - 1)
                  : event.key === "ArrowLeft"
                    ? Math.max(index - 1, 0)
                    : event.key === "Home"
                      ? 0
                      : event.key === "End"
                        ? tabs.length - 1
                        : null;
              if (target === null || target === index) return;
              event.preventDefault();
              setView(tabs[target]);
              event.currentTarget.parentElement
                ?.querySelectorAll<HTMLButtonElement>("[role='tab']")
                [target]?.focus();
            }}
          >
            {tab === "active" ? `Active (${total})` : `Recent (${recentTotal})`}
          </button>
        ))}
      </div>

      {activeView ? (
        <div id="issues-panel-active" role="tabpanel" aria-labelledby="issues-tab-active">
          {rows.length === 0 ? (
            error !== null ? (
              <OperationResult level="error" className="my-4">
                {error}
              </OperationResult>
            ) : (
              <p className="py-6 text-center text-sm text-ink-muted">
                {loading ? "Loading active issues…" : "No active issues"}
              </p>
            )
          ) : (
            <ul className="space-y-1.5">
              {rows.map((row) => (
                <li
                  key={row.id}
                  className="group rounded-lg border border-border bg-surface p-3 text-xs"
                >
                  <div className="flex items-start justify-between gap-2">
                    <span className="font-semibold text-danger">Action needed</span>
                    <span className="flex shrink-0 items-center gap-2">
                      {row.recovery ? (
                        <Button
                          size="sm"
                          disabled={row.recovery.status !== "available"}
                          onClick={() => void recover(row.id)}
                        >
                          {row.recovery.status === "queued"
                            ? "Queued"
                            : row.recovery.status === "running"
                              ? "Running"
                              : row.recovery.label}
                        </Button>
                      ) : null}
                      {row.occurrenceCount > 1 ? (
                        <span className="text-ink-muted">×{row.occurrenceCount}</span>
                      ) : null}
                      <span className="text-ink-muted" title={`Last seen ${formatLocalMinute(row.lastSeenUtc)}`}>
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
        </div>
      ) : (
        <div id="issues-panel-recent" role="tabpanel" aria-labelledby="issues-tab-recent">
          {recentRows.length === 0 ? (
            recentError !== null ? (
              <OperationResult level="error" className="my-4">
                {recentError}
              </OperationResult>
            ) : (
              <p className="py-6 text-center text-sm text-ink-muted">
                {recentLoading ? "Loading recent notifications…" : "No recent notifications"}
              </p>
            )
          ) : (
            <ul className="space-y-1.5">
              {recentRows.map((row) => (
                <li key={row.id} className="rounded-lg border border-border bg-surface p-3 text-xs">
                  <div className="flex items-start justify-between gap-2">
                    <span className={row.level === "error" ? "font-semibold text-danger" : row.level === "warning" ? "font-semibold text-warning" : "font-semibold text-ink"}>
                      {row.level === "error" ? "Error" : row.level === "warning" ? "Warning" : "Notice"}
                    </span>
                    <span className="flex shrink-0 items-center gap-2 text-ink-muted">
                      {row.occurrenceCount > 1 ? <span>×{row.occurrenceCount}</span> : null}
                      <span title={`First seen ${formatLocalMinute(row.firstSeenUtc)}`}>
                        {formatLocalMinute(row.lastSeenUtc)}
                      </span>
                    </span>
                  </div>
                  {row.path ? <div className="mt-0.5 select-text break-all text-ink">{row.path}</div> : null}
                  <div className="mt-0.5 select-text break-words text-ink-muted">{row.message}</div>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </ModalShell>
  );
}
