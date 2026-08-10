import { stripUrl } from "../models/items";
import type { ItemDetail } from "../state/items-store";
import { formatLocalMinute } from "../utils/displayTime";

// The right pane's metadata tab: content facts, the resolved capture time
// with its source, and the full copy-path list — the live health check (1 copy
// = backups missing or a drive absent; more than the sync factor = a
// misdetection worth a look).

function formatBytes(bytes: number | null): string {
  if (bytes === null) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[unit]}`;
}

function formatTaken(detail: ItemDetail): string {
  if (detail.resolvedUtcMs === null) return "Undated";
  const stamp = formatLocalMinute(detail.resolvedUtcMs);
  const suffix = detail.dateOnly ? " (date only)" : "";
  const source = detail.resolvedSource ? ` · ${detail.resolvedSource}` : "";
  return `${stamp}${suffix}${source}`;
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="mb-1">
      <dt className="text-xs text-ink-muted">{label}</dt>
      <dd className="break-words text-sm text-ink">{value}</dd>
    </div>
  );
}

export default function MetadataPane({
  detail,
  hash,
}: {
  detail: ItemDetail | null;
  hash: string | null;
}) {
  if (detail === null) {
    return <p className="p-3 text-sm text-ink-muted">No selection</p>;
  }
  return (
    {/* The parent pane is the sole scroller; a second overflow here would
        produce a double scrollbar the moment a height constraint lands. */}
    <dl className="p-3">
      <Row label="Name" value={detail.fileName} />
      <Row label="Taken" value={formatTaken(detail)} />
      <Row label="Size" value={formatBytes(detail.byteSize)} />
      {detail.kind === "video" && hash !== null && (detail.stripFrames ?? 0) > 0 ? (
        <div className="mb-2">
          <dt className="text-xs text-ink-muted">Snapshots</dt>
          <dd className="mt-1 flex flex-col gap-1">
            {Array.from({ length: detail.stripFrames ?? 0 }, (_, i) => (
              <img
                key={i}
                src={stripUrl(hash, i)}
                alt={`snapshot ${i + 1}`}
                loading="lazy"
                className="w-full rounded border border-border"
              />
            ))}
          </dd>
        </div>
      ) : null}
      {detail.width !== null && detail.height !== null ? (
        <Row label="Dimensions" value={`${detail.width} × ${detail.height}`} />
      ) : null}
      {detail.durationMs !== null ? (
        <Row label="Duration" value={`${Math.round(detail.durationMs / 1000)} s`} />
      ) : null}
      <div className="mb-1 mt-3">
        <dt className="text-xs text-ink-muted">
          Copies ({detail.copyPaths.length})
        </dt>
        {detail.copyPaths.map((path) => (
          <dd key={path} className="break-all py-0.5 text-xs text-ink" title={path}>
            {path}
          </dd>
        ))}
      </div>
      {detail.companionPaths.length > 0 ? (
        <div className="mb-1">
          <dt className="text-xs text-ink-muted">
            Companions ({detail.companionPaths.length})
          </dt>
          {detail.companionPaths.map((path) => (
            <dd key={path} className="break-all py-0.5 text-xs text-ink" title={path}>
              {path}
            </dd>
          ))}
        </div>
      ) : null}
    </dl>
  );
}
