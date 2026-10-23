import { useId, useRef } from "react";
import { useBlockingSurface } from "../hooks/useBlockingSurface";
import { useI18n } from "../i18n/I18nContext";
import Button from "./ui/Button";
import IdentifyingText from "./ui/IdentifyingText";
import BoundedText from "./ui/BoundedText";

// Missing roots and substituted roots have different safety meanings. An
// absent root is an availability problem: Main stays usable and the source
// pass continues with independent roots. A substituted root is a physical
// identity conflict, so the app must not act on it until the user resolves it.

export function MissingSourcesNotice({
  missing,
  onRecheck,
  onReconfigure,
}: {
  missing: string[];
  onRecheck: () => void;
  onReconfigure: () => void;
}) {
  const { t } = useI18n();
  return (
    <section
      aria-label={t("source.missingRegionLabel")}
      className="shrink-0 border-b border-warning/40 bg-warning-surface px-4 py-3 text-sm"
    >
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h2 className="font-semibold text-ink-strong">
            {t("source.missingTitle")}
          </h2>
          <p className="mt-0.5 text-ink-muted">{t("source.missingBody")}</p>
          {/* A banner names each folder on one line, keeping its final
              segment. */}
          <BoundedText className="mt-2 max-h-20 text-xs text-ink">
            <ul>
              {missing.map((path) => (
                <li key={path}>
                  <IdentifyingText value={path} path />
                </li>
              ))}
            </ul>
          </BoundedText>
        </div>
        <div className="flex shrink-0 gap-2">
          <Button onClick={onReconfigure}>
            {t("app.settings")}
          </Button>
          <Button variant="primary" onClick={onRecheck}>
            {t("app.checkSources")}
          </Button>
        </div>
      </div>
    </section>
  );
}

export function SubstitutedSourceGate({
  substituted,
  unknown,
  onRecheck,
  onReconfigure,
}: {
  substituted: string[];
  /** True when the check that would have answered "verified" or
   * "substituted" instead failed outright. Blocks exactly like a known
   * substitution: an unreadable check is never treated as "verified safe". */
  unknown: boolean;
  onRecheck: () => void;
  onReconfigure: () => void;
}) {
  const surfaceRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useBlockingSurface(surfaceRef);
  const { t } = useI18n();
  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-background p-6">
      <div
        ref={surfaceRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        className="flex max-h-[90vh] w-[min(820px,calc(100vw-3rem))] flex-col rounded-2xl border border-border bg-surface shadow-xl"
      >
        <header className="shrink-0 border-b border-control-edge px-5 py-3">
          <h1 id={titleId} className="text-lg font-semibold text-ink-strong">
            {t(unknown ? "source.substitutedCheckFailedTitle" : "source.substitutedTitle")}
          </h1>
        </header>
        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
          <p className="mb-3 text-sm text-ink-muted">
            {t(unknown ? "source.substitutedCheckFailedBody" : "source.substitutedBody")}
          </p>
          {unknown ? null : (
            <ul>
              {substituted.map((path) => (
                <li
                  key={path}
                  className="mb-1.5 break-all rounded-lg bg-danger-surface px-3 py-2 text-sm leading-relaxed text-danger"
                >
                  {path}
                </li>
              ))}
            </ul>
          )}
        </div>
        <footer className="flex shrink-0 justify-end gap-2 border-t border-control-edge px-5 py-3">
          <Button size="md" onClick={onReconfigure}>
            {t("source.rerunSetup")}
          </Button>
          <Button autoFocus variant="primary" size="md" onClick={onRecheck}>
            {t("app.checkSources")}
          </Button>
        </footer>
      </div>
    </div>
  );
}
