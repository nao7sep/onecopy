import { useWizardStore } from "../state/wizard-store";
import { useBlockingSurface } from "../hooks/useBlockingSurface";
import DirectoryRow from "./DirectoryRow";
import Button from "./ui/Button";
import { Plus } from "lucide-react";

const WIZARD_STEPS = 3;

// The Setup surface: three steps, blocking by design. There is deliberately
// NO install page (developer, 2026-08-17): Managed tools is the app's one
// install surface, and the warning-tinted footer chip funnels there the
// moment the scan meets a video or HEIC it cannot decode — a second install
// UI here would only drift from it.
//
// A FIRST run is completable only — there is nothing behind it until source
// directories exist, so it owes no Close affordance. A RE-RUN is different:
// the app is already configured and the user may simply be looking, so it
// offers Cancel, which writes nothing (every step edits store state, and
// `finish` is the sole writer).

export default function Wizard({ dataRoot }: { dataRoot: string }) {
  const step = useWizardStore((s) => s.step);
  const dirs = useWizardStore((s) => s.dirs);
  const timezone = useWizardStore((s) => s.timezone);
  const timezoneValid = useWizardStore((s) => s.timezoneValid);
  const cacheDir = useWizardStore((s) => s.cacheDir);
  const reconfigure = useWizardStore((s) => s.reconfigure);
  const addDirs = useWizardStore((s) => s.addDirs);
  const removeDir = useWizardStore((s) => s.removeDir);
  const setStep = useWizardStore((s) => s.setStep);
  const setTimezone = useWizardStore((s) => s.setTimezone);
  const pickCacheDir = useWizardStore((s) => s.pickCacheDir);
  const finish = useWizardStore((s) => s.finish);
  const cancel = useWizardStore((s) => s.cancel);

  useBlockingSurface();

  /** A re-run offers Cancel on EVERY page (developer, 2026-08-17 — being
   * three pages deep is no reason to walk back out first), beside Back on
   * steps 2+. A first run keeps neither on page 1: it is completable only,
   * with nothing behind it to return to. */
  const leading = (
    <span className="flex items-center gap-2">
      {reconfigure ? (
        <Button variant="ghost" onClick={cancel}>
          Cancel
        </Button>
      ) : null}
      {step > 1 ? (
        <Button variant="ghost" onClick={() => setStep((step - 1) as 1 | 2)}>
          Back
        </Button>
      ) : null}
    </span>
  );

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-background p-6">
      <div className="w-[560px] max-w-full rounded-2xl border border-border bg-surface p-7 shadow-xl">
        <h1 className="text-xl font-semibold tracking-tight text-ink-strong">
          {reconfigure ? "Reconfigure" : "Setup"}
        </h1>
        <p className="mt-1 mb-6 text-sm text-ink-muted">
          {`Step ${step} of ${WIZARD_STEPS}`}
        </p>

        {step === 1 ? (
          <section>
            <h2 className="mb-1 text-sm font-semibold text-ink-strong">
              Directories to handle
            </h2>
            <p className="mb-3 text-sm text-ink-muted">
              Everything in these folders is indexed, deduped, and offered for
              culling. Nothing is ever changed without you asking.
            </p>
            <ul className="mb-4 max-h-64 space-y-1.5 overflow-y-auto">
              {dirs.map((dir) => (
                <li key={dir.path}>
                  <DirectoryRow path={dir.path} onRemove={() => removeDir(dir.path)} />
                </li>
              ))}
            </ul>
            <Button className="mb-6" onClick={() => void addDirs()}>
              <Plus size={14} />
              Add directory
            </Button>
            <div className="flex items-center justify-between">
              {leading}
              <Button variant="primary" disabled={dirs.length === 0} onClick={() => setStep(2)}>
                Next
              </Button>
            </div>
          </section>
        ) : null}

        {step === 2 ? (
          <section>
            <h2 className="mb-1 text-sm font-semibold text-ink-strong">Default timezone</h2>
            <p className="mb-3 text-sm text-ink-muted">
              Applied when a photo&apos;s metadata has no timezone of its own
              (most cameras). IANA name, e.g. Asia/Tokyo.
            </p>
            <input
              className="mb-1 h-9 w-full rounded-lg border border-border bg-background px-3 text-sm text-ink outline-none transition-colors focus:border-border-strong focus-visible:ring-2 focus-visible:ring-primary-ring"
              value={timezone}
              onChange={(e) => void setTimezone(e.target.value)}
            />
            <p className="mb-6 min-h-4 text-xs text-danger">
              {timezoneValid ? "" : "Not a recognized timezone name"}
            </p>
            <div className="flex items-center justify-between">
              {leading}
              <Button
                variant="primary"
                disabled={!timezoneValid || timezone.trim() === ""}
                onClick={() => setStep(3)}
              >
                Next
              </Button>
            </div>
          </section>
        ) : null}

        {step === 3 ? (
          <section>
            <h2 className="mb-1 text-sm font-semibold text-ink-strong">Cache location</h2>
            <p className="mb-3 text-sm text-ink-muted">
              Thumbnails and fast previews live here — put it on your fastest
              disk. It rebuilds itself if you ever delete it.
            </p>
            <p className="mb-4 break-all rounded-lg border border-border bg-surface-muted/40 px-3 py-2 text-sm leading-relaxed text-ink">
              {cacheDir ?? `${dataRoot}/cache (default)`}
            </p>
            <Button className="mb-6" onClick={() => void pickCacheDir()}>
              Change
            </Button>
            <div className="flex items-center justify-between">
              {leading}
              <Button variant="primary" onClick={() => void finish()}>
                Finish and scan
              </Button>
            </div>
          </section>
        ) : null}
      </div>
    </div>
  );
}
