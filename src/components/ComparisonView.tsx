import { useEffect } from "react";
import ComparisonSlot from "./ComparisonSlot";
import {
  slotIndexForKey,
  slotIndexForShiftedCode,
  chunkSlots,
  pageCountOf,
  turnSize,
  visibleSlots,
  liveSlotCount,
  gridColumns,
  useComparisonStore,
} from "../state/comparison-store";
import { hasOpenModal } from "../utils/modalStack";
import ConfirmDialog from "./ConfirmDialog";

// The similar-photos comparison surface in the main window. With extra
// monitors the slot list spreads: this surface shows chunk 0 and the
// per-monitor windows show the rest, all sharing one global key space.
// The paged model (Phase 33): keys mark keepers (marks persist across
// pages), ←/→ page freely, S shows the shortlist of marks, Enter advances
// through unseen pages then commits the whole group — keep the marked,
// trash the rest (Shift = permanent) — and Escape leaves with nothing
// deleted. Nothing on an unvisited page can ever be deleted.

export default function ComparisonView() {
  const open = useComparisonStore((s) => s.open);
  const members = useComparisonStore((s) => s.members);
  const kept = useComparisonStore((s) => s.kept);
  const page = useComparisonStore((s) => s.page);
  const shortlist = useComparisonStore((s) => s.shortlist);
  const shortlistPage = useComparisonStore((s) => s.shortlistPage);
  const busy = useComparisonStore((s) => s.busy);
  const spreadCount = useComparisonStore((s) => s.spreadCount);
  const capacities = useComparisonStore((s) => s.capacities);
  const toggleKeep = useComparisonStore((s) => s.toggleKeep);
  const commitTurn = useComparisonStore((s) => s.commitTurn);
  const close = useComparisonStore((s) => s.close);
  const pendingPermanentCommit = useComparisonStore((s) => s.pendingPermanentCommit);
  const pendingCommitState = useComparisonStore((s) => s.pendingCommit);
  const commitFailure = useComparisonStore((s) => s.commitFailure);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      // A modal above the comparison (help, settings) owns the keyboard:
      // its Escape must close only itself, never tear down the session.
      if (hasOpenModal()) return;
      const unlinkIndex = slotIndexForShiftedCode(event);
      if (unlinkIndex >= 0) {
        event.preventDefault();
        void useComparisonStore.getState().unlinkSlot(unlinkIndex);
        return;
      }
      const slotIndex = slotIndexForKey(event);
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
      } else if (event.key === "ArrowRight" || event.key === "PageDown") {
        event.preventDefault();
        useComparisonStore.getState().nextPage();
      } else if (event.key === "ArrowLeft" || event.key === "PageUp") {
        event.preventDefault();
        useComparisonStore.getState().prevPage();
      } else if (event.key.toLowerCase() === "s" && !event.metaKey && !event.ctrlKey) {
        event.preventDefault();
        useComparisonStore.getState().toggleShortlist();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [open, toggleKeep, commitTurn, close]);

  if (!open) return null;

  const state = useComparisonStore.getState();
  const visible = visibleSlots(state);
  const size = turnSize(capacities);
  const pages = pageCountOf(members.length, size);
  const unseen = pages - state.visited.size;
  const chunks = chunkSlots(visible, kept, capacities);
  const localChunk = chunks[0] ?? [];
  const perChunk = localChunk.length;
  const portraitDominant = useComparisonStore.getState().portraitDominant;
  const columns = gridColumns(
    Math.max(1, perChunk),
    window.innerWidth / Math.max(1, window.innerHeight),
    portraitDominant,
  );
  const rows = Math.max(1, Math.ceil(Math.max(1, perChunk) / columns));

  return (
    <div className="fixed inset-0 z-20 flex flex-col bg-background">
      {pendingCommitState !== null ? (
        <ConfirmDialog
          title={pendingCommitState.keepCount === 0 ? "Trash the whole group?" : "Commit this group?"}
          message={
            pendingCommitState.keepCount === 0
              ? `Nothing is marked to keep — move all ${pendingCommitState.trashCount} photos to the trash?`
              : `Keep ${pendingCommitState.keepCount} and move ${pendingCommitState.trashCount} to the trash${
                  pendingCommitState.permanent ? " (PERMANENT delete)" : ""
                }?`
          }
          confirmLabel={pendingCommitState.keepCount === 0 ? "Trash all" : "Commit"}
          onConfirm={() => void useComparisonStore.getState().confirmPendingCommit()}
          onCancel={() => useComparisonStore.getState().cancelPendingCommit()}
        />
      ) : null}
      {pendingPermanentCommit ? (
        <ConfirmDialog
          title="Delete permanently this session?"
          message="Shift+Enter commits will PERMANENTLY delete the non-kept photos, bypassing the trash. Confirm once for this comparison session — every later Shift+Enter here acts without asking again."
          confirmLabel="Delete permanently"
          onConfirm={() => void useComparisonStore.getState().confirmPermanentCommit()}
          onCancel={() => useComparisonStore.getState().cancelPermanentCommit()}
        />
      ) : null}
      <header className="flex shrink-0 items-center justify-between border-b border-border bg-surface px-3 py-2">
        <h1 className="text-sm font-semibold text-ink-strong">Similar photos</h1>
        <div className="flex items-center gap-4 text-xs text-ink-muted">
          <span>
            {shortlist
              ? `Shortlist ${shortlistPage + 1}/${pageCountOf(
                  members.filter((m) => m !== null && kept.has(m.hash)).length,
                  size,
                )} · ${kept.size} kept`
              : `Page ${page + 1}/${pages} · ${kept.size} kept · ${liveSlotCount(
                  members,
                )} photos`}
            {spreadCount > 0 ? ` across ${spreadCount + 1} screens` : ""}
            {!shortlist && unseen > 0 ? ` · ${unseen} page${unseen === 1 ? "" : "s"} unseen` : ""}
          </span>
          <span>
            Keys keep · Shift+key not similar · Left/Right pages · S shortlist ·
            Enter {unseen > 0 && !shortlist ? "next page" : "commits (Shift = permanent)"} ·
            Escape leaves
          </span>
          <button
            className="rounded border border-border px-2 py-0.5 text-ink hover:bg-surface-muted"
            onClick={close}
          >
            Close
          </button>
        </div>
      </header>
      <div
        className="grid min-h-0 flex-1 gap-3 p-3"
        style={{
          gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`,
          gridTemplateRows: `repeat(${rows}, minmax(0, 1fr))`,
        }}
      >
        {localChunk.map((slot, index) =>
          slot.member !== null ? (
            <ComparisonSlot
              key={slot.member.hash}
              member={slot.member}
              slotKey={slot.slotKey}
              kept={slot.kept}
              onToggle={() => toggleKeep(index)}
              onUnlink={() => void useComparisonStore.getState().unlinkSlot(index)}
            />
          ) : (
            <EmptySlot key={`empty-${slot.slotKey}`} slotKey={slot.slotKey} />
          ),
        )}
        {perChunk === 0 ? (
          <p className="m-auto text-ink-muted">All slots are on the other screens</p>
        ) : null}
      </div>
      {busy ? (
        <footer className="shrink-0 border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
          Working…
        </footer>
      ) : null}
      {commitFailure !== null ? (
        <footer className="flex shrink-0 items-center justify-between gap-3 border-t border-danger/30 bg-danger-surface px-3 py-1 text-xs text-danger">
          <span>{commitFailure.message}</span>
          <button
            className="rounded border border-danger/40 px-2 py-0.5 hover:bg-danger/10"
            onClick={() => void commitTurn(commitFailure.permanent)}
          >
            Retry remaining
          </button>
        </footer>
      ) : null}
    </div>
  );
}

/** An unlinked slot's place for the rest of the turn: keeping the hole is
 * what keeps every other slot's key number true. The dimmed key label says
 * which number this was. */
function EmptySlot({ slotKey }: { slotKey: string }) {
  return (
    <div className="flex h-full w-full items-center justify-center rounded-lg border-2 border-dashed border-border">
      <span className="text-lg font-bold text-ink-muted opacity-40">
        {slotKey.toUpperCase()}
      </span>
    </div>
  );
}
