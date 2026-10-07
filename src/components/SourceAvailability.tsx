import { useBlockingSurface } from "../hooks/useBlockingSurface";
import { useI18n } from "../i18n/I18nContext";
import Button from "./ui/Button";

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
          <ul className="mt-2 max-h-20 overflow-y-auto text-xs text-ink">
            {missing.map((path) => (
              <li key={path} className="break-all">
                {path}
              </li>
            ))}
          </ul>
        </div>
        <div className="flex shrink-0 gap-2">
          <Button onClick={onReconfigure}>
            {t("app.settings")}
          </Button>
          <Button variant="primary" onClick={onRecheck}>
            {t("source.checkAgain")}
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
   * substitution: an unreadable check is never treated as "verified safe"
   * (R3-07). */
  unknown: boolean;
  onRecheck: () => void;
  onReconfigure: () => void;
}) {
  useBlockingSurface();
  const { t } = useI18n();
  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-background p-6">
      <div className="w-[min(820px,calc(100vw-3rem))] rounded-2xl border border-border bg-surface p-7 shadow-xl">
        <h1 className="mb-1 text-lg font-semibold text-ink-strong">
          {t(unknown ? "source.substitutedCheckFailedTitle" : "source.substitutedTitle")}
        </h1>
        <p className="mb-3 text-sm text-ink-muted">
          {t(unknown ? "source.substitutedCheckFailedBody" : "source.substitutedBody")}
        </p>
        {unknown ? null : (
          <ul className="mb-4 max-h-64 overflow-y-auto">
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
        <div className="flex justify-end gap-2">
          <Button size="md" onClick={onReconfigure}>
            {t("source.rerunSetup")}
          </Button>
          <Button variant="primary" size="md" onClick={onRecheck}>
            {t("source.checkAgain")}
          </Button>
        </div>
      </div>
    </div>
  );
}
