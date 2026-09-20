import { Fragment } from "react";
import { useI18n } from "../i18n/I18nContext";
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
  const { t } = useI18n();
  if (!open) return null;

  return (
    <ModalShell
      title={t("shortcuts.title")}
      onClose={onClose}
      widthClass="w-[min(1160px,calc(100vw-3rem))]"
    >
      <p className="mb-5 text-xs text-ink-muted">
        {t("shortcuts.intro")}
      </p>
      {/* The track minimum decides how many columns FIT, not how wide they end
          up — the 1fr share does that — so it is set low enough that all three
          stand up inside the surface the shell actually grants at the window's
          own opening width, gutter and 16px scrollbar included. At 19rem they
          did not, and the third column wrapped under the first two. */}
      <div className="grid grid-cols-[repeat(auto-fit,minmax(min(100%,17rem),1fr))] gap-8">
        {shortcutColumns().map((column) => (
          <div key={column[0].title} className="min-w-0 space-y-7" data-shortcut-column>
          {column.map((group) => (
          <section key={group.title} aria-label={t(group.title)}>
          <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {t(group.title)}
            {/* The scope, stated: a chord pressed with the wrong surface
                focused looks broken rather than scoped. */}
            <span className="mt-1 block font-normal normal-case tracking-normal text-ink-muted/70">
              {t(group.context)}
            </span>
          </h2>
          <dl className="space-y-2.5">
            {group.rows.map((row) => (
              <div key={`${row.chord}-${row.action}`} className="flex items-baseline gap-3">
                <dd className="min-w-0 flex-1 text-sm text-ink">{t(row.action)}</dd>
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
