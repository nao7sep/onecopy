import { useEffect } from "react";
import { previewUrl } from "../models/items";
import {
  SLOT_KEYS,
  useComparisonStore,
  type GroupMember,
} from "../state/comparison-store";

// The similar-photos comparison surface: every group member at once, one slot
// key each. Press keys to mark keepers, Enter to commit the turn (trash the
// rest; Shift+Enter deletes permanently), Escape or Close to leave. This is
// the single-window form; the multi-screen spread reuses the same store.

function Slot({
  member,
  slotKey,
  kept,
  onToggle,
}: {
  member: GroupMember;
  slotKey: string;
  kept: boolean;
  onToggle: () => void;
}) {
  return (
    <figure
      className={`relative w-[23%] min-w-56 cursor-default rounded border-2 p-1 ${
        kept ? "border-primary bg-primary-surface" : "border-border bg-surface"
      }`}
      onClick={onToggle}
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

export default function ComparisonView() {
  const open = useComparisonStore((s) => s.open);
  const slots = useComparisonStore((s) => s.slots);
  const queue = useComparisonStore((s) => s.queue);
  const kept = useComparisonStore((s) => s.kept);
  const busy = useComparisonStore((s) => s.busy);
  const toggleKeep = useComparisonStore((s) => s.toggleKeep);
  const commitTurn = useComparisonStore((s) => s.commitTurn);
  const close = useComparisonStore((s) => s.close);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      const key = event.key.toLowerCase();
      const slotIndex = (SLOT_KEYS as readonly string[]).indexOf(key);
      if (slotIndex >= 0) {
        event.preventDefault();
        toggleKeep(slotIndex);
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        void commitTurn(event.shiftKey);
      } else if (event.key === "Escape") {
        event.preventDefault();
        close();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [open, toggleKeep, commitTurn, close]);

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-20 flex flex-col bg-background">
      <header className="flex shrink-0 items-center justify-between border-b border-border bg-surface px-3 py-2">
        <h1 className="text-sm font-semibold text-ink-strong">Similar photos</h1>
        <div className="flex items-center gap-4 text-xs text-ink-muted">
          <span>
            {kept.size} kept · {slots.length} shown
            {queue.length > 0 ? ` · ${queue.length} waiting` : ""}
          </span>
          <span>Keys 1–9/0/A–F keep · Enter commits (Shift = permanent) · Escape leaves</span>
          <button
            className="rounded border border-border px-2 py-0.5 text-ink hover:bg-surface-muted"
            onClick={close}
          >
            Close
          </button>
        </div>
      </header>
      <div className="flex min-h-0 flex-1 flex-wrap content-start gap-3 overflow-y-auto p-3">
        {slots.map((member, index) => (
          <Slot
            key={member.hash}
            member={member}
            slotKey={SLOT_KEYS[index] ?? "?"}
            kept={kept.has(member.hash)}
            onToggle={() => toggleKeep(index)}
          />
        ))}
      </div>
      {busy ? (
        <footer className="shrink-0 border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
          Working…
        </footer>
      ) : null}
    </div>
  );
}
