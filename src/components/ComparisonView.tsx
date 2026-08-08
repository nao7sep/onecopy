import { useEffect } from "react";
import ComparisonSlot from "./ComparisonSlot";
import {
  SLOT_KEYS,
  chunkSlots,
  useComparisonStore,
} from "../state/comparison-store";

// The similar-photos comparison surface in the main window. With extra
// monitors the slot list spreads: this surface shows chunk 0 and the
// per-monitor windows show the rest, all sharing one global key space.
// Keys mark keepers, Enter commits the turn (trash the rest; Shift+Enter
// deletes permanently), Escape or Close leaves.

export default function ComparisonView() {
  const open = useComparisonStore((s) => s.open);
  const slots = useComparisonStore((s) => s.slots);
  const queue = useComparisonStore((s) => s.queue);
  const kept = useComparisonStore((s) => s.kept);
  const busy = useComparisonStore((s) => s.busy);
  const spreadCount = useComparisonStore((s) => s.spreadCount);
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

  const chunks = chunkSlots(slots, kept, 1 + spreadCount);
  const localChunk = chunks[0] ?? [];
  const perChunk = localChunk.length;

  return (
    <div className="fixed inset-0 z-20 flex flex-col bg-background">
      <header className="flex shrink-0 items-center justify-between border-b border-border bg-surface px-3 py-2">
        <h1 className="text-sm font-semibold text-ink-strong">Similar photos</h1>
        <div className="flex items-center gap-4 text-xs text-ink-muted">
          <span>
            {kept.size} kept · {slots.length} shown
            {spreadCount > 0 ? ` across ${spreadCount + 1} screens` : ""}
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
        {localChunk.map((slot, index) => (
          <ComparisonSlot
            key={slot.member.hash}
            member={slot.member}
            slotKey={slot.slotKey}
            kept={slot.kept}
            onToggle={() => toggleKeep(index)}
          />
        ))}
        {perChunk === 0 ? (
          <p className="m-auto text-ink-muted">All slots are on the other screens</p>
        ) : null}
      </div>
      {busy ? (
        <footer className="shrink-0 border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
          Working…
        </footer>
      ) : null}
    </div>
  );
}
