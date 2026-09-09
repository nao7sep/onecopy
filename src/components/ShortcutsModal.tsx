import { Fragment } from "react";
import { shortcutColumns } from "../models/shortcuts";
import ModalShell from "./ModalShell";

// The shortcuts help surface: a named modal, grouped by area, opened by
// Cmd+Slash (Question as alias), labelled Close + Escape.
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
    <ModalShell
      title="Keyboard shortcuts"
      onClose={onClose}
      widthClass="w-[min(1160px,calc(100vw-3rem))]"
    >
      <p className="mb-5 text-xs text-ink-muted">
        Shortcuts pause during text composition. Focused controls keep their normal keys.
        On macOS, Ctrl aliases yield to text editing; Cmd remains the command key.
      </p>
      <div className="grid grid-cols-[repeat(auto-fit,minmax(min(100%,19rem),1fr))] gap-8">
        {shortcutColumns().map((column) => (
          <div key={column[0].title} className="min-w-0 space-y-7" data-shortcut-column>
          {column.map((group) => (
          <section key={group.title} aria-label={group.title}>
          <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {group.title}
            {/* The scope, stated: a chord pressed with the wrong surface
                focused looks broken rather than scoped. */}
            <span className="mt-1 block font-normal normal-case tracking-normal text-ink-muted/70">
              {group.context}
            </span>
          </h2>
          <dl className="space-y-2.5">
            {group.rows.map((row) => (
              <div key={`${row.chord}-${row.action}`} className="flex items-baseline gap-3">
                <dd className="min-w-0 flex-1 text-sm text-ink">{row.action}</dd>
                <dt className="min-w-0 max-w-[48%] shrink-0">
                  <kbd className="inline-block max-w-full rounded-md border border-border bg-surface-muted px-2 py-1 text-center font-mono text-xs text-ink-strong [overflow-wrap:anywhere]">
                    {row.chord.split("/").map((part, index) => (
                      <Fragment key={index}>
                        {index > 0 ? <>/<wbr /></> : null}
                        <span className="whitespace-nowrap">{part}</span>
                      </Fragment>
                    ))}
                  </kbd>
                </dt>
              </div>
            ))}
          </dl>
          </section>
          ))}
          </div>
        ))}
      </div>
    </ModalShell>
  );
}
