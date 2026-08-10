import { useEffect } from "react";
import { useIssuesStore } from "../state/issues-store";
import { formatLocalMinute } from "../utils/displayTime";

// The issues list: newest first, plain rows. Read-only by design — issues are
// the pipeline's testimony, not a work queue.

export default function IssuesView() {
  const rows = useIssuesStore((s) => s.rows);
  const total = useIssuesStore((s) => s.total);
  const load = useIssuesStore((s) => s.load);

  useEffect(() => {
    void load();
  }, [load]);

  if (rows.length === 0) {
    return <p className="m-auto text-ink-muted">No issues</p>;
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto p-3">
      <p className="mb-2 shrink-0 text-xs text-ink-muted">
        {total} issue{total === 1 ? "" : "s"}
        {total > rows.length ? ` (showing the latest ${rows.length})` : ""}
      </p>
      <ul>
        {rows.map((row) => (
          <li key={row.id} className="mb-1 rounded border border-border bg-surface p-2 text-xs">
            <div className="flex justify-between gap-2">
              <span className="font-semibold text-danger">{row.kind}</span>
              <span className="shrink-0 text-ink-muted">
                {formatLocalMinute(row.createdAtUtc)}
              </span>
            </div>
            {row.path ? (
              <div className="break-all text-ink" title={row.path}>
                {row.path}
              </div>
            ) : null}
            {row.message ? (
              <div className="break-words text-ink-muted">{row.message}</div>
            ) : null}
          </li>
        ))}
      </ul>
    </div>
  );
}
