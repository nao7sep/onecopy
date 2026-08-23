import { useEffect, useRef, useState } from "react";
import { emit } from "@tauri-apps/api/event";
import { listenThenAnnounce } from "../utils/handshake";
import ComparisonSlot from "../components/ComparisonSlot";
import ZoomableImage from "../components/ZoomableImage";
import { gridColumns, type ComparisonBroadcast } from "../state/comparison-store";

// A secondary comparison surface on an extra monitor: renders its contiguous
// slice of the slot list with GLOBAL slot keys, forwards every relevant key to
// the main window (which owns all mutations), and closes when the main window
// says the comparison is over (its window is simply closed by the store).

export default function ComparisonWindow({ slice }: { slice: number }) {
  const [state, setState] = useState<ComparisonBroadcast | null>(null);
  const [enlarged, setEnlarged] = useState<{ hash: string; name: string } | null>(null);
  const enlargedRef = useRef(enlarged);
  enlargedRef.current = enlarged;

  useEffect(() => {
    // Announce only once this window can hear the reply (see handshake.ts).
    const unlisten = listenThenAnnounce<ComparisonBroadcast>(
      "comparison://state",
      "comparison://ready",
      setState,
    );
    const onKeyDown = (event: KeyboardEvent) => {
      if (enlargedRef.current !== null) {
        // The enlarged overlay owns the keyboard locally: Escape returns to
        // the slots (never forwarded — it must not close the comparison);
        // Z falls through to the zoom toggle.
        if (event.key === "Escape") {
          event.preventDefault();
          setEnlarged(null);
        }
        return;
      }
      event.preventDefault();
      // Modifiers ride along: without them the receiver cannot tell Cmd+0
      // from a bare slot-0 press, and forwarding strips the distinction.
      void emit("comparison://key", {
        key: event.key,
        // The physical key: Shift+digit delivers a symbol in `key`, so the
        // unlink chord can only be recognized from `code`.
        code: event.code,
        shiftKey: event.shiftKey,
        metaKey: event.metaKey,
        ctrlKey: event.ctrlKey,
        altKey: event.altKey,
      });
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      unlisten();
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  const chunk = state?.chunks[slice] ?? [];
  // GEOMETRY FROM THE PAGE, NOT THIS SCREEN'S OCCUPANCY (Phase 33): every
  // window of a page uses the grid of its FULLEST chunk, so a lone photo on
  // the second screen renders as one cell of the same grid the first screen
  // shows — never a giant full-screen image beside a 4-up (the developer's
  // 5-member case). Empty cells render as cells.
  const gridSlots = Math.max(1, ...(state?.chunks ?? []).map((c) => c.length));
  const columns = gridColumns(
    gridSlots,
    window.innerWidth / Math.max(1, window.innerHeight),
    state?.portraitDominant ?? false,
  );
  const rows = Math.max(1, Math.ceil(gridSlots / columns));

  // A claimed screen whose chunk emptied (the last page is short) goes PURE
  // BLACK — the curtain that says "still in comparison mode". "Waiting…"
  // belongs only to the moment before the FIRST broadcast arrives.
  if (state !== null && chunk.length === 0) {
    return <div className="h-screen w-screen bg-black" />;
  }

  return (
    <div className="flex h-screen flex-col bg-background">
      <div
        className="grid min-h-0 flex-1 gap-3 p-3"
        style={{
          gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`,
          gridTemplateRows: `repeat(${rows}, minmax(0, 1fr))`,
        }}
      >
        {chunk.length === 0 ? (
          <p className="m-auto text-ink-muted">Waiting for the comparison…</p>
        ) : (
          chunk.map((slot) =>
            slot.member !== null ? (
              <ComparisonSlot
                key={slot.member.hash}
                member={slot.member}
                slotKey={slot.slotKey}
                kept={slot.kept}
                onToggle={() =>
                  void emit("comparison://key", { key: slot.slotKey, shiftKey: false })
                }
                onEnlarge={() =>
                  setEnlarged({ hash: slot.member!.hash, name: slot.member!.fileName })
                }
              />
            ) : (
              <div
                key={`empty-${slot.slotKey}`}
                className="flex h-full w-full items-center justify-center rounded-lg border-2 border-dashed border-border"
              >
                <span className="text-lg font-bold text-ink-muted opacity-40">
                  {slot.slotKey.toUpperCase()}
                </span>
              </div>
            ),
          )
        )}
      </div>
      <footer className="shrink-0 border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
        Keys work here too · Enter commits · Escape leaves
      </footer>
      {enlarged !== null ? (
        <div className="absolute inset-0 z-10 flex flex-col bg-background">
          <div className="flex min-h-0 flex-1 items-center justify-center overflow-hidden p-2">
            <ZoomableImage hash={enlarged.hash} fileName={enlarged.name} />
          </div>
          <footer className="shrink-0 border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
            {enlarged.name} · Z: 100% · Escape: back to the slots
          </footer>
        </div>
      ) : null}
    </div>
  );
}
