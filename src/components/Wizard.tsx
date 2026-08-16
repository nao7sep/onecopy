import { useWizardStore } from "../state/wizard-store";
import { useBinariesStore } from "../state/binaries-store";
import { useBlockingSurface } from "../hooks/useBlockingSurface";
import DirectoryRow from "./DirectoryRow";
import Button from "./ui/Button";
import { Plus } from "lucide-react";

/** The wizard's numbered steps. The ffmpeg screen follows as an OFFER, not a
 * fourth step, so it is deliberately not counted here. */
const WIZARD_STEPS = 3;

// The Setup surface: three steps and one offer, blocking by design.
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

  /** Back on steps 2+; Cancel on step 1 of a re-run; nothing on a first run's
   * first step, where there is no state to return to. */
  const leading =
    step > 1 ? (
      <Button variant="ghost" onClick={() => setStep((step - 1) as 1 | 2 | 3)}>
        Back
      </Button>
    ) : reconfigure ? (
      <Button variant="ghost" onClick={cancel}>
        Cancel
      </Button>
    ) : (
      <span />
    );

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-background p-6">
      <div className="w-[560px] max-w-full rounded-2xl border border-border bg-surface p-7 shadow-xl">
        <h1 className="text-xl font-semibold tracking-tight text-ink-strong">
          {reconfigure ? "Reconfigure" : "Setup"}
        </h1>
        {/* Three STEPS and one OFFER (Design: First-run wizard). The offer is
            step 4 in the flow's own counter but is not a fourth step, so it
            must not read "Step 4 of 3" — it announces itself as optional
            instead, which is also the honest signal that Finish is reachable
            from here without doing anything. */}
        <p className="mt-1 mb-6 text-sm text-ink-muted">
          {step <= WIZARD_STEPS ? `Step ${step} of ${WIZARD_STEPS}` : "Optional"}
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
              <Button variant="primary" onClick={() => setStep(4)}>
                Next
              </Button>
            </div>
          </section>
        ) : null}

        {step === 4 ? (
          <section>
            {/* The one OFFER (not a step): skippable, and skipping degrades
                honestly — placeholder tiles plus the Managed tools path. */}
            <h2 className="mb-1 text-sm font-semibold text-ink-strong">ffmpeg</h2>
            {/* Leads with VIDEO deliberately. Framing this as an iPhone
                feature told anyone with a camera full of clips that it did not
                apply to them. */}
            <p className="mb-4 text-sm text-ink-muted">
              Needed for videos and for HEIC photos. Free, managed by OneCopy,
              and you can install it later from the menu.
            </p>
            <FfmpegOfferRow />
            <div className="mt-6 flex items-center justify-between">
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

/** The ffmpeg status + install control inside the wizard offer. An install
 * started here keeps running after Finish — the footer chip carries its
 * progress, so the offer never blocks the scan. */
function FfmpegOfferRow() {
  const state = useBinariesStore((s) => s.state);
  const installing = useBinariesStore((s) => s.installing);
  const progress = useBinariesStore((s) => s.progress);
  const install = useBinariesStore((s) => s.install);
  const installed = state !== null && state.status !== "not-installed";
  return (
    <div className="flex items-center justify-between gap-3 rounded-lg border border-border bg-surface-muted/40 px-3 py-2 text-sm">
      <span className="text-ink">
        {installing
          ? progress
          : installed
            ? `Installed (${state?.facts.installedVersion ?? "unknown version"})`
            : "Not installed"}
      </span>
      {!installed && !installing ? (
        <Button variant="primary" onClick={() => void install()}>
          Install
        </Button>
      ) : null}
    </div>
  );
}
