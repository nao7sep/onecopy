import { useEffect } from "react";
import { primaryModWord } from "../utils/shortcuts";

// The shortcuts help surface: a named modal, grouped by area, chords written
// per the keyboard-shortcut-conventions with the runtime platform's modifier
// word. Opened by Cmd+Slash (Question as alias); labelled Close + Escape.

function Row({ chord, action }: { chord: string; action: string }) {
  return (
    <div className="flex justify-between gap-4 py-0.5 text-sm">
      <span className="shrink-0 font-mono text-ink-strong">{chord}</span>
      <span className="text-right text-ink-muted">{action}</span>
    </div>
  );
}

function Group({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="mb-3">
      <h2 className="mb-1 text-xs font-semibold uppercase text-ink-muted">{title}</h2>
      {children}
    </section>
  );
}

export default function ShortcutsModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [open, onClose]);

  if (!open) return null;
  const mod = primaryModWord();

  return (
    <div className="fixed inset-0 z-30 flex items-center justify-center bg-background/80">
      <div className="w-[440px] max-w-[90vw] rounded border border-border bg-surface p-4">
        <div className="mb-3 flex items-center justify-between">
          <h1 className="text-sm font-semibold text-ink-strong">Keyboard shortcuts</h1>
          <button
            className="rounded border border-border px-2 py-0.5 text-xs text-ink hover:bg-surface-muted"
            onClick={onClose}
          >
            Close
          </button>
        </div>
        <Group title="Grid">
          <Row
            chord="Delete/Backspace"
            action="Trash the item and every copy (Shift: delete permanently)"
          />
          <Row chord="Enter" action="Compare similar photos, or open the preview" />
        </Group>
        <Group title="Comparison view">
          <Row chord="1–9 / 0 / A–F" action="Keep the photo in that slot" />
          <Row chord="Enter" action="Commit the turn (Shift+Enter: delete permanently)" />
          <Row chord="Escape" action="Leave without committing" />
        </Group>
        <Group title="Preview window">
          <Row chord="Escape" action="Close the preview window" />
        </Group>
        <Group title="Zoom">
          <Row chord={`${mod}+Plus/Minus`} action="Zoom in / out" />
          <Row chord={`${mod}+0`} action="Reset zoom" />
        </Group>
        <Group title="Help">
          <Row chord={`${mod}+Slash / Question`} action="This help" />
        </Group>
      </div>
    </div>
  );
}
