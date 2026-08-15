import { useWizardStore } from "../state/wizard-store";
import { useBinariesStore } from "../state/binaries-store";
import { useBlockingSurface } from "../hooks/useBlockingSurface";

/** The wizard's numbered steps. The ffmpeg screen follows as an OFFER, not a
 * fourth step, so it is deliberately not counted here. */
const WIZARD_STEPS = 3;

// The first-run Setup surface: three steps, blocking by design (there is
// nothing behind it until source directories exist), completable only —
// not dismissable, so no Close affordance is owed.

function totalEstimateGb(
  dirs: { counts: { images: number; videos: number } | null }[],
): number {
  // Coarse preview-cache estimate: ~0.25 MB per image preview+thumb, videos
  // get posters/strips later at roughly half that.
  const megabytes = dirs.reduce((sum, dir) => {
    if (!dir.counts) return sum;
    return sum + dir.counts.images * 0.25 + dir.counts.videos * 0.12;
  }, 0);
  return Math.round((megabytes / 1024) * 10) / 10;
}

export default function Wizard({ dataRoot }: { dataRoot: string }) {
  const step = useWizardStore((s) => s.step);
  const dirs = useWizardStore((s) => s.dirs);
  const timezone = useWizardStore((s) => s.timezone);
  const timezoneValid = useWizardStore((s) => s.timezoneValid);
  const cacheDir = useWizardStore((s) => s.cacheDir);
  const addDirs = useWizardStore((s) => s.addDirs);
  const removeDir = useWizardStore((s) => s.removeDir);
  const setStep = useWizardStore((s) => s.setStep);
  const setTimezone = useWizardStore((s) => s.setTimezone);
  const pickCacheDir = useWizardStore((s) => s.pickCacheDir);
  const finish = useWizardStore((s) => s.finish);

  const counting = dirs.some((d) => d.counting);

  useBlockingSurface();

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-background">
      <div className="w-[560px] max-w-[90vw] rounded border border-border bg-surface p-6">
        <h1 className="mb-1 text-lg font-semibold text-ink-strong">Setup</h1>
        {/* Three STEPS and one OFFER (Design: First-run wizard). The offer is
            step 4 in the flow's own counter but is not a fourth step, so it
            must not read "Step 4 of 3" — it announces itself as optional
            instead, which is also the honest signal that Finish is reachable
            from here without doing anything. */}
        <p className="mb-4 text-sm text-ink-muted">
          {step <= WIZARD_STEPS ? `Step ${step} of ${WIZARD_STEPS}` : "Optional"}
        </p>

        {step === 1 ? (
          <section>
            <h2 className="mb-2 text-sm font-semibold text-ink-strong">
              Directories to handle
            </h2>
            <ul className="mb-3 max-h-64 overflow-y-auto">
              {dirs.map((dir) => (
                <li
                  key={dir.path}
                  className="mb-1 flex items-center justify-between gap-2 rounded border border-border px-2 py-1"
                >
                  <span className="min-w-0 flex-1 truncate text-sm text-ink" title={dir.path}>
                    {dir.path}
                  </span>
                  <span className="shrink-0 text-xs text-ink-muted">
                    {dir.counting
                      ? "scanning…"
                      : dir.counts
                        ? `${dir.counts.images} images · ${dir.counts.videos} videos · ${dir.counts.others} other`
                        : "—"}
                  </span>
                  <button
                    className="shrink-0 text-xs text-danger"
                    onClick={() => removeDir(dir.path)}
                  >
                    Remove
                  </button>
                </li>
              ))}
            </ul>
            <button
              className="mb-4 rounded border border-border px-2 py-1 text-sm text-primary hover:bg-primary-surface"
              onClick={() => void addDirs()}
            >
              Add directory…
            </button>
            <div className="flex justify-end">
              <button
                className="rounded bg-primary px-3 py-1 text-sm text-ink-inverted disabled:bg-surface-muted disabled:text-ink-muted"
                disabled={dirs.length === 0}
                onClick={() => setStep(2)}
              >
                Next
              </button>
            </div>
          </section>
        ) : null}

        {step === 2 ? (
          <section>
            <h2 className="mb-2 text-sm font-semibold text-ink-strong">
              Default timezone
            </h2>
            <p className="mb-2 text-sm text-ink-muted">
              Applied when a photo's metadata has no timezone of its own (most
              cameras). IANA name, e.g. Asia/Tokyo.
            </p>
            <input
              className="mb-1 w-full rounded border border-border bg-background px-2 py-1 text-sm text-ink"
              value={timezone}
              onChange={(e) => void setTimezone(e.target.value)}
            />
            {!timezoneValid ? (
              <p className="mb-2 text-xs text-danger">Not a recognized timezone name</p>
            ) : (
              <p className="mb-2 text-xs text-ink-muted">&nbsp;</p>
            )}
            <div className="flex justify-between">
              <button
                className="rounded border border-border px-3 py-1 text-sm text-ink"
                onClick={() => setStep(1)}
              >
                Back
              </button>
              <button
                className="rounded bg-primary px-3 py-1 text-sm text-ink-inverted disabled:bg-surface-muted disabled:text-ink-muted"
                disabled={!timezoneValid || timezone.trim() === ""}
                onClick={() => setStep(3)}
              >
                Next
              </button>
            </div>
          </section>
        ) : null}

        {step === 3 ? (
          <section>
            <h2 className="mb-2 text-sm font-semibold text-ink-strong">Cache location</h2>
            <p className="mb-2 text-sm text-ink-muted">
              Thumbnails and fast previews live here — put it on your fastest
              disk. Estimated size for the added directories: ~
              {totalEstimateGb(dirs)} GB.
            </p>
            <p className="mb-2 break-all rounded border border-border px-2 py-1 text-sm text-ink">
              {cacheDir ?? `${dataRoot}/cache (default)`}
            </p>
            <button
              className="mb-4 rounded border border-border px-2 py-1 text-sm text-primary hover:bg-primary-surface"
              onClick={() => void pickCacheDir()}
            >
              Change…
            </button>
            <div className="flex justify-between">
              <button
                className="rounded border border-border px-3 py-1 text-sm text-ink"
                onClick={() => setStep(2)}
              >
                Back
              </button>
              <button
                className="rounded bg-primary px-3 py-1 text-sm text-ink-inverted"
                onClick={() => setStep(4)}
              >
                Next
              </button>
            </div>
          </section>
        ) : null}
        {step === 4 ? (
          <section>
            {/* The one OFFER (not a step): skippable, and skipping degrades
                honestly — placeholder tiles plus the Managed tools path. */}
            <h2 className="mb-2 text-sm font-semibold text-ink-strong">ffmpeg</h2>
            <p className="mb-2 text-sm text-ink-muted">
              iPhone photos (HEIC) and video posters decode through ffmpeg,
              which OneCopy downloads and manages itself (free, one click).
              For an iPhone-heavy library it is effectively required. Skipping
              is fine: those files show placeholders until you install it from
              the menu&apos;s Managed tools.
            </p>
            <FfmpegOfferRow />
            <div className="mt-4 flex justify-between">
              <button
                className="rounded border border-border px-3 py-1 text-sm text-ink"
                onClick={() => setStep(3)}
              >
                Back
              </button>
              <button
                className="rounded bg-primary px-3 py-1 text-sm text-ink-inverted disabled:bg-surface-muted disabled:text-ink-muted"
                disabled={counting}
                title={counting ? "Waiting for directory counts" : undefined}
                onClick={() => void finish()}
              >
                Finish and scan
              </button>
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
    <div className="flex items-center justify-between rounded border border-border px-2 py-1 text-sm">
      <span className="text-ink">
        {installing
          ? progress
          : installed
            ? `ffmpeg ${state?.facts.installedVersion ?? ""} is installed`
            : "ffmpeg is not installed"}
      </span>
      {!installed && !installing ? (
        <button
          className="rounded bg-primary px-2 py-0.5 text-xs text-ink-inverted"
          onClick={() => void install()}
        >
          Install ffmpeg
        </button>
      ) : null}
    </div>
  );
}
