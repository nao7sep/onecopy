import { previewUrl } from "../models/items";
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
  return (
    <figure
      className={`relative w-[23%] min-w-56 cursor-pointer rounded border-2 p-1 ${
        kept ? "border-primary bg-primary-surface" : "border-border bg-surface"
      }`}
      onClick={onToggle}
      onDoubleClick={onEnlarge}
      title="Click: keep · double-click: enlarge"
    >
      <div className="flex h-64 items-center justify-center overflow-hidden">
        <img
          src={previewUrl(member.hash)}
          alt={member.fileName}
          className="max-h-full max-w-full object-contain"
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
      <figcaption className="mt-1 flex justify-between text-xs text-ink-muted">
        <span className="truncate" title={member.fileName}>
          {member.fileName}
        </span>
        {member.sharpness !== null ? (
          <span title="Sharpness (advisory)">{Math.round(member.sharpness)}</span>
        ) : null}
      </figcaption>
    </figure>
  );
}
