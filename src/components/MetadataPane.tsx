import { FolderOpen } from "lucide-react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { formatBytes, stripUrl } from "../models/items";
import type { ItemDetail } from "../state/items-store";
import { formatLocalMinute } from "../utils/displayTime";
import { fileManagerWord } from "../utils/shortcuts";
import { log, toErrorFields } from "../repositories";

// The right pane's metadata tab: content facts, the resolved capture time
// with its source, and the full copy-path list — the live health check (1 copy
// = backups missing or a drive absent; more than the sync factor = a
// misdetection worth a look).

/** Every copy is revealable individually. A logical item can live on four
 * drives, so "show me the file" has no single answer — the button belongs on
 * each PATH, which is also the only place the user can say which copy they
 * meant. */
function PathRow({ path }: { path: string }) {
  const word = fileManagerWord();
  return (
    <dd className="group flex items-start gap-1 py-0.5">
      <span className="min-w-0 flex-1 break-all text-xs text-ink" title={path}>
        {path}
      </span>
      <button
        aria-label={`Show in ${word}`}
        title={`Show in ${word}`}
        className="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded text-ink-muted opacity-0 transition-opacity hover:bg-surface-muted hover:text-ink focus-visible:opacity-100 group-hover:opacity-100"
        onClick={() => {
          void revealItemInDir(path).catch((error) => {
            // A copy on an unplugged drive is the ordinary failure here, and
            // it is worth a log line rather than silence.
            log.warn("reveal failed", { path, ...toErrorFields(error) });
          });
        }}
      >
        <FolderOpen size={13} />
      </button>
    </dd>
  );
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
  // The parent pane is the sole scroller; a second overflow here would
  // produce a double scrollbar the moment a height constraint lands.
  return (
    <dl className="p-3">
      <Row label="Name" value={detail.fileName} />
      <Row label="Taken" value={formatTaken(detail)} />
      <Row
        label="Size"
        value={detail.byteSize !== null ? formatBytes(detail.byteSize) : "—"}
      />
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
          <PathRow key={path} path={path} />
        ))}
      </div>
      {detail.companionPaths.length > 0 ? (
        <div className="mb-1">
          <dt className="text-xs text-ink-muted">
            Companions ({detail.companionPaths.length})
          </dt>
          {detail.companionPaths.map((path) => (
            <PathRow key={path} path={path} />
          ))}
        </div>
      ) : null}
    </dl>
  );
}
