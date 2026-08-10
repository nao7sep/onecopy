import { primaryModWord } from "../utils/shortcuts";
import ModalShell from "./ModalShell";

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
  if (!open) return null;
  const mod = primaryModWord();

  return (
    <ModalShell title="Keyboard shortcuts" onClose={onClose} widthClass="w-[460px]">
      <Group title="Grid">
        <Row
          chord="Delete/Backspace"
          action="Trash the item and every copy (Shift: delete permanently)"
        />
        <Row chord="Enter" action="Compare similar photos, or open the preview" />
        <Row chord="P" action="Toggle preview-follows-selection" />
      </Group>
      <Group title="Destinations tree">
        <Row chord="Enter" action="Move the selection here, trash the other copies" />
        <Row chord="Shift+Enter" action="Move here, permanently delete the rest" />
        <Row chord={`${mod}+Enter`} action="Copy here, leave everything in place" />
      </Group>
      <Group title="Comparison view">
        <Row chord="1–9/0/A–F" action="Keep the photo in that slot" />
        <Row chord="Enter" action="Commit the turn (Shift+Enter: delete permanently)" />
        <Row chord="Double-click" action="Enlarge one slot (Escape returns)" />
        <Row chord="Escape" action="Leave without committing" />
      </Group>
      <Group title="Preview window">
        <Row chord="Z" action="Toggle 100% view of the original" />
        <Row chord="F" action="Toggle fullscreen" />
        <Row chord="Escape" action="Close the preview window" />
      </Group>
      <Group title="Zoom">
        <Row chord={`${mod}+Equal/Plus/Semicolon`} action="Zoom in" />
        <Row chord={`${mod}+Minus`} action="Zoom out" />
        <Row chord={`${mod}+0`} action="Reset zoom" />
      </Group>
      <Group title="Help">
        <Row chord={`${mod}+Slash / Question`} action="This help" />
      </Group>
    </ModalShell>
  );
}
