import { useEffect, useRef, useState } from "react";
import { emit } from "@tauri-apps/api/event";
import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window";
import { isComposingEvent } from "../hooks/useComposing";
import { useComparisonLayout } from "../hooks/useComparisonLayout";
import { reportWindowCall } from "../repositories";
import { listenThenAnnounce } from "../utils/handshake";
import ComparisonSlot from "../components/ComparisonSlot";
import { type ComparisonBroadcast } from "../state/comparison-store";
import { gridFor } from "../models/comparisonSession";
import { hasOpenModal } from "../utils/modalStack";
import RevealCopiesDialog from "../components/RevealCopiesDialog";
import { comparisonKeyIsRoutable } from "../workflows/comparison";

// Secondary displays render one contiguous part of the current page. The main
// window remains the sole owner of selection and file operations.

export default function ComparisonWindow({ slice }: { slice: number }) {
  const itemArea = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<ComparisonBroadcast | null>(null);
  const [revealMember, setRevealMember] = useState<{
    hash: string;
    fileName: string;
  } | null>(null);
  const stateRef = useRef(state);
  stateRef.current = state;
  useComparisonLayout(itemArea, (state?.chunks[slice]?.length ?? 0) > 0, (aspect) => {
    void emit("comparison://layout", { slice, aspect }).catch(reportWindowCall("comparison layout"));
  });

  useEffect(() => {
    itemArea.current?.focus();
    const stopFocus = listenThenAnnounce<{ label: string }>("comparison://focus", "comparison://ready", ({ label }) => {
      if (label === getCurrentWindow().label) itemArea.current?.focus();
    });
    // Announce only once this window can hear the reply (see handshake.ts).
    const unlisten = listenThenAnnounce<ComparisonBroadcast>(
      "comparison://state",
      "comparison://ready",
      setState,
    );
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented || isComposingEvent(event) ||
        hasOpenModal() ||
        (event.target instanceof Element &&
          event.target.closest(
            "button, input, select, textarea, [contenteditable='true'], [role='menu']",
          ) !== null)
      ) {
        return;
      }
      const visibleCount =
        stateRef.current?.chunks.reduce(
          (count, chunk) => count + chunk.length,
          0,
        ) ?? 0;
      if (!comparisonKeyIsRoutable(event, visibleCount)) return;
      event.preventDefault();
      event.stopPropagation();
      const message = {
        key: event.key,
        repeat: event.repeat,
        shiftKey: event.shiftKey,
        metaKey: event.metaKey,
        ctrlKey: event.ctrlKey,
        altKey: event.altKey,
        returnWindow: getCurrentWindow().label,
      };
      void (event.key === " " ? currentMonitor() : Promise.resolve(null))
        .then((monitor) => emit("comparison://key", { ...message, monitor: monitor ?? undefined }))
        .catch(reportWindowCall("comparison key forwarding"));
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      unlisten();
      stopFocus();
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  const chunk = state?.chunks[slice] ?? [];
  const offset = (state?.chunks ?? [])
    .slice(0, slice)
    .reduce((count, members) => count + members.length, 0);
  const grid = gridFor(
    chunk.length,
    state?.portraitDominant ?? false,
    state?.displayAspects[slice],
    state?.capacities[slice] ?? null,
  );

  if (state !== null && chunk.length === 0) {
    return <div className="h-screen w-screen bg-black" />;
  }

  return (
    <div className="flex h-screen flex-col bg-background">
      {revealMember !== null ? (
        <RevealCopiesDialog
          hash={revealMember.hash}
          fileName={revealMember.fileName}
          onClose={() => setRevealMember(null)}
        />
      ) : null}
      <div
        ref={itemArea}
        tabIndex={0}
        role="listbox"
        aria-label="Images on the current comparison display"
        aria-multiselectable="true"
        className="grid min-h-0 flex-1 grid-flow-row gap-3 p-3"
        style={{
          gridTemplateColumns: `repeat(${grid.columns}, minmax(0, 1fr))`,
          gridTemplateRows: `repeat(${grid.rows}, minmax(0, 1fr))`,
        }}
      >
        {chunk.length === 0 ? (
          <p className="m-auto text-ink-muted">Waiting for the comparison…</p>
        ) : (
          chunk.map((slot, index) => (
            <ComparisonSlot
              key={slot.member.hash}
              member={slot.member}
              slotKey={slot.slotKey}
              marked={slot.marked}
              anchor={slot.anchor}
              onSelect={(mode) => {
                itemArea.current?.focus();
                void emit("comparison://select", {
                  slotIndex: offset + index,
                  mode,
                }).catch(reportWindowCall("comparison card selection"));
              }}
              onReveal={() =>
                setRevealMember({
                  hash: slot.member.hash,
                  fileName: slot.member.fileName,
                })
              }
            />
          ))
        )}
      </div>
      <footer className="shrink-0 border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
        0–9, A–Z, or Keep toggle marks · Space opens the picked image · Arrows inspect · Enter reviews
        marked keepers and the visible Trash set · Escape closes
      </footer>
    </div>
  );
}
