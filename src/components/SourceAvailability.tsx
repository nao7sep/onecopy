import { useBlockingSurface } from "../hooks/useBlockingSurface";

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
  return (
    <section
      aria-label="Unavailable source folders"
      className="shrink-0 border-b border-warning/40 bg-warning-surface px-4 py-3 text-sm"
    >
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h2 className="font-semibold text-ink-strong">
            Some source folders are unavailable
          </h2>
          <p className="mt-0.5 text-ink-muted">
            OneCopy will keep working with available files. Connect the missing
            drive and check again, or re-run setup to remove or replace a folder.
          </p>
          <ul className="mt-2 max-h-20 overflow-y-auto text-xs text-ink">
            {missing.map((path) => (
              <li key={path} className="break-all">
                {path}
              </li>
            ))}
          </ul>
        </div>
        <div className="flex shrink-0 gap-2">
          <button
            className="inline-flex h-8 items-center justify-center rounded-md border border-border bg-surface px-3 font-medium text-ink hover:bg-surface-muted"
            onClick={onReconfigure}
          >
            Re-run setup…
          </button>
          <button
            className="inline-flex h-8 items-center justify-center rounded-md bg-primary px-3 font-medium text-ink-inverted hover:brightness-110"
            onClick={onRecheck}
          >
            Check again
          </button>
        </div>
      </div>
    </section>
  );
}

export function SubstitutedSourceGate({
  substituted,
  onRecheck,
  onReconfigure,
}: {
  substituted: string[];
  onRecheck: () => void;
  onReconfigure: () => void;
}) {
  useBlockingSurface();
  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-background p-6">
      <div className="w-[min(820px,calc(100vw-3rem))] rounded-2xl border border-border bg-surface p-7 shadow-xl">
        <h1 className="mb-1 text-lg font-semibold text-ink-strong">
          Different source volume detected
        </h1>
        <p className="mb-3 text-sm text-ink-muted">
          A configured folder is present on a different physical volume than
          the one OneCopy recorded. Mount the original drive, or re-run setup
          and remove then add the folder if this change is intentional.
        </p>
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
        <div className="flex justify-end gap-2">
          <button
            className="inline-flex h-9 items-center justify-center rounded-lg border border-border bg-surface px-4 text-sm font-medium text-ink hover:bg-surface-muted"
            onClick={onReconfigure}
          >
            Re-run setup…
          </button>
          <button
            className="inline-flex h-9 items-center justify-center rounded-lg bg-primary px-4 text-sm font-medium text-ink-inverted shadow-sm outline-none transition-all hover:brightness-110 focus-visible:ring-2 focus-visible:ring-primary-ring"
            onClick={onRecheck}
          >
            Check again
          </button>
        </div>
      </div>
    </div>
  );
}
