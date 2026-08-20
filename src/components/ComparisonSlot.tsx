import { factsLine, previewUrl } from "../models/items";
import type { GroupMember } from "../state/comparison-store";
import { Focus, Smile } from "lucide-react";

// One comparison slot, shared by the main comparison surface and the
// secondary per-monitor windows: preview image, the GLOBAL slot key, keeper
// highlight, copy badge, sharpness hint.

export default function ComparisonSlot({
  member,
  slotKey,
  kept,
  onToggle,
  onUnlink,
  onEnlarge,
}: {
  member: GroupMember;
  slotKey: string;
  kept: boolean;
  onToggle: () => void;
  /** "Not the same subject": removes this image from the similar set,
   * permanently and non-destructively. Absent in surfaces that cannot
   * mutate the session (the secondary windows forward keys instead). */
  onUnlink?: () => void;
  onEnlarge?: () => void;
}) {
  const facts = factsLine(member);
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
      onClick={onToggle}
      onDoubleClick={onEnlarge}
      title="Click: keep · double-click: enlarge"
    >
      <div className="flex min-h-0 flex-1 items-center justify-center overflow-hidden">
        <img
          src={previewUrl(member.hash)}
          alt={member.fileName}
          className="h-full w-full object-contain"
        />
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
      <figcaption className="mt-1 shrink-0 text-xs text-ink-muted">
        <span className="flex justify-between gap-2">
          <span className="truncate text-ink" title={member.fileName}>
            {member.fileName}
          </span>
          <span className="flex shrink-0 gap-2">
            {/* Face score beside sharpness — both advisory. Only a real face
                (> 0) earns the badge; scored-faceless and unscored show
                nothing, so face-free libraries never see the column. */}
            {member.faceScore !== null && member.faceScore > 0 ? (
              <span className="flex items-center gap-1" title="Face score (advisory)">
                <Smile size={12} aria-hidden="true" /> {Math.round(member.faceScore * 100)}
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
