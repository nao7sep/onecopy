import { useWizardStore } from "../state/wizard-store";
import { useBlockingSurface } from "../hooks/useBlockingSurface";

// The session gate: configured source directories that are not currently
// present block work mode — this app never reasons about partial presence.
// Blocking surface, named, completable only by making the volumes available
// (or fixing config by hand) and re-checking.

export default function PresenceGate({
  missing,
  substituted,
}: {
  missing: string[];
  substituted: string[];
}) {
  const recheck = useWizardStore((s) => s.recheckPresence);
  useBlockingSurface();
  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-background p-6">
      <div className="w-[min(820px,calc(100vw-3rem))] rounded-2xl border border-border bg-surface p-7 shadow-xl">
        <h1 className="mb-1 text-lg font-semibold text-ink-strong">
          Volumes missing
        </h1>
        <p className="mb-3 text-sm text-ink-muted">
          These configured directories are not available. Connect the drives
          they live on, then check again — work is blocked until every
          configured directory is present.
        </p>
        <ul className="mb-4 max-h-64 overflow-y-auto">
          {missing.map((path) => (
            <li
              key={path}
              className="mb-1.5 break-all rounded-lg bg-danger-surface px-3 py-2 text-sm leading-relaxed text-danger"
            >
              {path}
            </li>
          ))}
          {substituted.map((path) => (
            <li
              key={path}
              className="mb-1.5 break-all rounded-lg bg-danger-surface px-3 py-2 text-sm leading-relaxed text-danger"
            >
              {path}
              <span className="mt-0.5 block text-xs">
                Present, but on a DIFFERENT volume than recorded — a swapped
                drive with the same folder layout. Mount the original drive,
                or remove and re-add this directory if the change is
                intentional.
              </span>
            </li>
          ))}
        </ul>
        <div className="flex justify-end">
          <button
            className="inline-flex h-9 items-center justify-center rounded-lg bg-primary px-4 text-sm font-medium text-ink-inverted shadow-sm outline-none transition-all hover:brightness-110 focus-visible:ring-2 focus-visible:ring-primary-ring"
            onClick={() => void recheck()}
          >
            Check again
          </button>
        </div>
      </div>
    </div>
  );
}
