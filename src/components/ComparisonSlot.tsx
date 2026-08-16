import { factsLine, previewUrl } from "../models/items";
import type { GroupMember } from "../state/comparison-store";

// One comparison slot, shared by the main comparison surface and the
// secondary per-monitor windows: preview image, the GLOBAL slot key, keeper
// highlight, copy badge, sharpness hint.

export default function ComparisonSlot({
  member,
  slotKey,
  kept,
  onToggle,
  onEnlarge,
}: {
  member: GroupMember;
  slotKey: string;
  kept: boolean;
  onToggle: () => void;
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
      className={`relative flex h-full min-h-0 w-full cursor-pointer flex-col rounded-lg border-2 p-1 ${
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
      <figcaption className="mt-1 shrink-0 text-xs text-ink-muted">
        <span className="flex justify-between gap-2">
          <span className="truncate text-ink" title={member.fileName}>
            {member.fileName}
          </span>
          {member.sharpness !== null ? (
            <span className="shrink-0" title="Sharpness (advisory)">
              ◐ {Math.round(member.sharpness)}
            </span>
          ) : null}
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
