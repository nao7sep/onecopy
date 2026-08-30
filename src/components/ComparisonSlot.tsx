import { factsLine } from "../models/items";
import { faceStarLabel, faceStarRating } from "../models/itemPresentation";
import type { GroupMember } from "../state/comparison-store";
import { useState } from "react";
import { ExternalLink, Focus } from "lucide-react";
import InspectableImage from "./InspectableImage";
import { useAppStore } from "../state/app-store";
import { openInDefaultApp } from "../workflows/external-open";
import { log, toErrorFields } from "../repositories";

// One comparison slot, shared by the main comparison surface and the
// secondary per-monitor windows: preview image, the GLOBAL slot key, keeper
// highlight, copy badge, sharpness hint.

export default function ComparisonSlot({
  member,
  slotKey,
  kept,
  onToggle,
  onUnlink,
}: {
  member: GroupMember;
  slotKey: string;
  kept: boolean;
  onToggle: () => void;
  /** "Not the same subject": removes this image from the similar set,
   * permanently and non-destructively. Absent in surfaces that cannot
   * mutate the session (the secondary windows forward keys instead). */
  onUnlink?: () => void;
}) {
  const [externalError, setExternalError] = useState(false);
  const facts = factsLine(member);
  const showFaceStars = useAppStore(
    (state) => state.appData?.config?.showFaceStars !== false,
  );
  const faceStars = showFaceStars ? faceStarRating(member.faceScore) : 0;
  return (
    <figure
      // Assistive tech learns the keep state; the slot stays out of the Tab
      // order by design — the direct-address keys (1–9/0/A–F) span windows
      // where a roving tabindex cannot exist.
      role="button"
      aria-pressed={kept}
      aria-label={`Slot ${slotKey}: ${member.fileName}`}
      // Fills its GRID CELL: the fixed-width wrapping tiles left every image
      // small however much screen there was — the cell is as big as the
      // count allows, so the image is too.
      className={`group/slot relative flex h-full min-h-0 w-full cursor-pointer flex-col rounded-lg border-2 p-1 ${
        kept ? "border-primary bg-primary-surface" : "border-border bg-surface"
      }`}
      onClick={(event) => {
        if (event.detail === 1) onToggle();
      }}
      title="Click or double-click: toggle keep"
    >
      <div className="flex min-h-0 flex-1 items-center justify-center overflow-hidden">
        <InspectableImage hash={member.hash} fileName={member.fileName} enlargeSmall />
      </div>
      <span
        className={`absolute left-2 top-2 flex h-8 w-8 items-center justify-center rounded text-lg font-bold ${
          kept ? "bg-primary text-ink-inverted" : "bg-surface-muted text-ink-strong"
        }`}
      >
        {slotKey.toUpperCase()}
      </span>
      {member.copyCount > 1 ? (
        <span className="absolute right-2 top-2 rounded bg-primary-surface px-1 text-xs text-primary">
          ×{member.copyCount}
        </span>
      ) : null}
      {onUnlink ? (
        <button
          className="absolute bottom-8 right-2 hidden rounded-md bg-surface-muted px-1.5 py-0.5 text-xs text-ink-muted hover:text-ink group-hover/slot:block"
          title="Not similar — remove from this set (Shift+slot key). The photo is not deleted."
          onClick={(event) => {
            event.stopPropagation();
            onUnlink();
          }}
        >
          Not similar
        </button>
      ) : null}
      <button
        className="absolute bottom-8 left-2 hidden rounded-md bg-surface-muted p-1 text-ink-muted hover:text-ink focus-visible:block group-hover/slot:block"
        aria-label={`Open ${member.fileName} in default app`}
        title="Open in default app"
        onClick={(event) => {
          event.stopPropagation();
          setExternalError(false);
          void openInDefaultApp(member.hash, null).catch((error) => {
            log.warn("comparison external open failed", toErrorFields(error));
            setExternalError(true);
          });
        }}
      >
        <ExternalLink size={14} />
      </button>
      {externalError ? (
        <span className="absolute bottom-8 left-10 rounded bg-background px-1 text-xs text-danger">
          Couldn’t open
        </span>
      ) : null}
      <figcaption className="mt-1 shrink-0 text-xs text-ink-muted">
        <span className="flex justify-between gap-2">
          <span className="truncate text-ink" title={member.fileName}>
            {member.fileName}
          </span>
          <span className="flex shrink-0 gap-2">
            {/* A score is a best-face confidence/smile hint, not a percentage
                rating. Zero/no-face and unscored stay quiet. */}
            {faceStars > 0 ? (
              <span
                className="tracking-tight text-primary"
                title={faceStarLabel(faceStars)}
              >
                <span aria-hidden="true">{"★".repeat(faceStars)}</span>
                <span className="sr-only">{faceStarLabel(faceStars)}</span>
              </span>
            ) : null}
            {member.sharpness !== null ? (
              <span className="flex items-center gap-1" title="Sharpness (advisory)">
                <Focus size={12} aria-hidden="true" /> {Math.round(member.sharpness)}
              </span>
            ) : null}
          </span>
        </span>
        {/* Pixels and bytes are what make the choice possible. A group is very
            often one shot at three qualities — original, export, web copy —
            and at slot size they are the same picture. */}
        {facts !== "" ? (
          <span className="mt-0.5 block tabular-nums">{facts}</span>
        ) : null}
      </figcaption>
    </figure>
  );
}
