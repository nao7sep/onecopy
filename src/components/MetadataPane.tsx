import { useEffect, useState } from "react";
import { FolderOpen, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  formatBytes,
  stripTimestampMs,
  stripUrl,
  thumbUrl,
  timestampLabel,
} from "../models/items";
import { itemKey, useItemsStore, type ItemDetail } from "../state/items-store";
import { useComparisonStore, type GroupMember } from "../state/comparison-store";
import { formatLocalMinute } from "../utils/displayTime";
import { fileManagerWord } from "../utils/shortcuts";
import { log, toErrorFields } from "../repositories";
import Button from "./ui/Button";
import TranscriptBlock from "./TranscriptBlock";
import { usePreviewStore } from "../state/preview-store";
import { useAppStore } from "../state/app-store";

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
    <dd className="group flex items-start gap-1.5 py-0.5">
      {/* A leading glyph plus a hanging indent: paths WRAP, and without a
          marker the wrapped lines read as separate entries — the developer
          could not see where one copy ended and the next began. */}
      <span aria-hidden className="mt-1 h-1 w-1 shrink-0 rounded-full bg-ink-muted" />
      <span className="min-w-0 flex-1 select-text break-all text-xs leading-relaxed text-ink" title={path}>
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

/** The similar-group strip: every member as a small thumb, the shown item
 * marked, click selecting that member in the grid (same section by
 * construction — groups are bucketed within one section), and Compare doing
 * exactly what Enter does. The group is the one fact about a photo that the
 * pane could not show at all. */
function SimilarSection({ hash }: { hash: string }) {
  const [members, setMembers] = useState<GroupMember[]>([]);
  // Bumped after an unlink so the list refetches without an anchor move.
  const [generation, setGeneration] = useState(0);
  useEffect(() => {
    let stale = false;
    void invoke<GroupMember[]>("get_similar_group", { hash })
      .then((rows) => {
        if (!stale) setMembers(rows);
      })
      .catch((error) => log.error("similar group load failed", toErrorFields(error)));
    return () => {
      stale = true;
    };
  }, [hash, generation]);
  if (members.length < 2) return null;
  return (
    <div className="mb-1 mt-3">
      <div className="flex items-center justify-between gap-2">
        <dt className="text-xs text-ink-muted">Similar ({members.length})</dt>
        <Button onClick={() => void useComparisonStore.getState().openGroup(hash)}>
          Compare
        </Button>
      </div>
      <dd className="mt-1 flex gap-1 overflow-x-auto pb-1">
        {members.map((member) => (
          <span key={member.hash} className="group/similar relative shrink-0">
            <button
              title={member.fileName}
              className={`h-20 w-20 shrink-0 overflow-hidden rounded-md border bg-background transition-colors ${
                member.hash === hash
                  ? "border-primary-ring ring-1 ring-primary-ring"
                  : "border-border hover:border-border-strong"
              }`}
              onClick={() => {
                // Select that member in the grid; the preview follows through
                // the ordinary anchor path.
                const { items, selectItem } = useItemsStore.getState();
                const target = items.find((i) => itemKey(i) === member.hash);
                if (target) selectItem(member.hash);
              }}
            >
              <img
                src={thumbUrl(member.hash)}
                alt={member.fileName}
                loading="lazy"
                className="h-full w-full object-contain"
              />
            </button>
            {/* The unlink where intruders are usually SPOTTED. Non-destructive
                and persistent: the pair never regroups on any later scan. */}
            <button
              className="absolute -right-1 -top-1 hidden h-4 w-4 items-center justify-center rounded-full bg-surface-muted text-[11px] leading-none text-ink-muted hover:text-danger group-hover/similar:flex"
              title={`Not similar — remove ${member.fileName} from this set. The photo is not deleted.`}
              onClick={() => {
                void invoke("similar_unlink", { hash: member.hash })
                  .then(() => setGeneration((n) => n + 1))
                  .catch((error) => {
                    log.warn("similar unlink failed", {
                      hash: member.hash,
                      ...toErrorFields(error),
                    });
                  });
              }}
            >
              <X size={12} />
            </button>
          </span>
        ))}
      </dd>
    </div>
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
      <dd className="select-text break-words text-sm text-ink">{value}</dd>
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
  const playAfterSnapshot = useAppStore(
    (state) => state.appData?.config?.videoAutoplayAfterSnapshot !== false,
  );
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
          <dd className="mt-1 flex gap-1 overflow-x-auto pb-1">
            {Array.from({ length: detail.stripFrames ?? 0 }, (_, i) => {
              const atMs = stripTimestampMs(
                detail.durationMs ?? 0,
                detail.stripFrames ?? 0,
                i,
              );
              return (
                <button
                  key={i}
                  className="relative h-20 w-20 shrink-0 overflow-hidden rounded border border-border bg-background hover:border-border-strong"
                  title={`Show video at ${timestampLabel(atMs)}`}
                  aria-label={`Show video at ${timestampLabel(atMs)}`}
                  onClick={() =>
                    void usePreviewStore
                      .getState()
                      .open(
                        { hash, pathId: null },
                        detail,
                        { seekMs: atMs, playAfterSeek: playAfterSnapshot },
                      )
                  }
                >
                  <img
                    src={stripUrl(hash, i)}
                    alt={`snapshot at ${timestampLabel(atMs)}`}
                    loading="lazy"
                    className="h-full w-full object-contain"
                  />
                  <span className="absolute bottom-0.5 right-0.5 rounded bg-background/80 px-1 text-[11px] text-ink">
                    {timestampLabel(atMs)}
                  </span>
                </button>
              );
            })}
          </dd>
        </div>
      ) : null}
      {detail.kind === "video" && hash !== null ? (
        <TranscriptBlock hash={hash} variant="compact" />
      ) : null}
      {detail.width !== null && detail.height !== null ? (
        <Row label="Dimensions" value={`${detail.width} × ${detail.height}`} />
      ) : null}
      {detail.durationMs !== null ? (
        <Row label="Duration" value={`${Math.round(detail.durationMs / 1000)} s`} />
      ) : null}
      {hash !== null && detail.kind === "image" ? <SimilarSection hash={hash} /> : null}
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
