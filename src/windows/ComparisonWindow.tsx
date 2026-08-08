import { useEffect, useState } from "react";
import { listen, emit } from "@tauri-apps/api/event";
import ComparisonSlot from "../components/ComparisonSlot";
import type { ComparisonBroadcast } from "../state/comparison-store";

// A secondary comparison surface on an extra monitor: renders its contiguous
// slice of the slot list with GLOBAL slot keys, forwards every relevant key to
// the main window (which owns all mutations), and closes when the main window
// says the comparison is over (its window is simply closed by the store).

export default function ComparisonWindow({ slice }: { slice: number }) {
  const [state, setState] = useState<ComparisonBroadcast | null>(null);

  useEffect(() => {
    const unlisten = listen<ComparisonBroadcast>("comparison://state", (event) => {
      setState(event.payload);
    });
    void emit("comparison://ready", {});
    const onKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      void emit("comparison://key", { key: event.key, shiftKey: event.shiftKey });
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      void unlisten.then((fn) => fn());
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  const chunk = state?.chunks[slice] ?? [];

  return (
    <div className="flex h-screen flex-col bg-background">
      <div className="flex min-h-0 flex-1 flex-wrap content-start gap-3 overflow-y-auto p-3">
        {chunk.length === 0 ? (
          <p className="m-auto text-ink-muted">Waiting for the comparison…</p>
        ) : (
          chunk.map((slot) => (
            <ComparisonSlot
              key={slot.member.hash}
              member={slot.member}
              slotKey={slot.slotKey}
              kept={slot.kept}
              onToggle={() =>
                void emit("comparison://key", { key: slot.slotKey, shiftKey: false })
              }
            />
          ))
        )}
      </div>
      <footer className="shrink-0 border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
        Keys work here too · Enter commits · Escape leaves
      </footer>
    </div>
  );
}
