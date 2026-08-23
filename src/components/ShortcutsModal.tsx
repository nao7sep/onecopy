import { shortcutGroups } from "../models/shortcuts";
import ModalShell from "./ModalShell";

// The shortcuts help surface: a named modal, grouped by area, opened by
// Cmd+Slash (Question as alias), labelled Close + Escape.
//
// Keys sit in a fixed right-hand column, as every app that shows a shortcut
// sheet does. They used to be on the left with the description ragged-right
// against them, which left the two columns interleaving and made the sheet
// read as prose rather than as a reference to scan.
//
// The rows themselves live in models/shortcuts.ts so the suite can walk them.

export default function ShortcutsModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  if (!open) return null;

  return (
    <ModalShell title="Keyboard shortcuts" onClose={onClose} widthClass="w-[520px]">
      {shortcutGroups().map((group) => (
        <section key={group.title} className="mb-5 last:mb-0">
          <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {group.title}
            {/* The scope, stated: a chord pressed with the wrong surface
                focused looks broken rather than scoped. */}
            <span className="ml-2 font-normal normal-case tracking-normal text-ink-muted/70">
              {group.context}
            </span>
          </h2>
          <dl className="space-y-1">
            {group.rows.map((row) => (
              <div key={`${row.chord}-${row.action}`} className="flex items-baseline gap-4">
                <dd className="min-w-0 flex-1 text-sm text-ink">{row.action}</dd>
                <dt className="shrink-0">
                  <kbd className="rounded-md border border-border bg-surface-muted px-2 py-0.5 font-mono text-xs text-ink-strong">
                    {row.chord}
                  </kbd>
                </dt>
              </div>
            ))}
          </dl>
        </section>
      ))}
    </ModalShell>
  );
}
