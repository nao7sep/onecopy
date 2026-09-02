import { useWizardStore } from "../state/wizard-store";
import { finishWizard } from "../workflows/wizard";
import { useBlockingSurface } from "../hooks/useBlockingSurface";
import DirectoryRow from "./DirectoryRow";
import Button from "./ui/Button";
import { Plus } from "lucide-react";
import { Row, Toggle } from "./ui/Field";
import {
  optionalFeatureSupported,
  type OptionalFeatureId,
} from "../models/optionalFeatures";
import { useId } from "react";
import OperationResult from "./ui/OperationResult";
import { useAppStore } from "../state/app-store";

const WIZARD_STEPS = 3;

// The Setup surface is a blocking root launch gate. There is deliberately
// NO install page (developer, 2026-08-17): Managed tools is the app's one
// install surface, and the warning-tinted footer chip funnels there the
// moment the scan meets a video or HEIC it cannot decode — a second install
// UI here would only drift from it.
//
// A FIRST run is completable only — there is nothing behind it until source
// directories exist, so it owes no Close affordance. A RE-RUN is different:
// the app is already configured and the user may simply be looking, so it
// offers Cancel, which writes nothing (every step edits store state, and
// the Finish workflow is the sole writer).

export default function Wizard() {
  const timezoneErrorId = useId();
  const step = useWizardStore((s) => s.step);
  const dirs = useWizardStore((s) => s.dirs);
  const timezone = useWizardStore((s) => s.timezone);
  const timezoneValid = useWizardStore((s) => s.timezoneValid);
  const timezonePending = useWizardStore((s) => s.timezonePending);
  const error = useWizardStore((s) => s.error);
  const reconfigure = useWizardStore((s) => s.reconfigure);
  const optionalFeatures = useWizardStore((s) => s.optionalFeatures);
  const optionalFeatureReasons = useWizardStore((s) => s.optionalFeatureReasons);
  const addDirs = useWizardStore((s) => s.addDirs);
  const removeDir = useWizardStore((s) => s.removeDir);
  const setStep = useWizardStore((s) => s.setStep);
  const setOptionalFeature = useWizardStore((s) => s.setOptionalFeature);
  const setTimezone = useWizardStore((s) => s.setTimezone);
  const cancel = useWizardStore((s) => s.cancel);
  const faceScoringSupported = useAppStore(
    (s) => s.appData?.faceScoringSupported === true,
  );
  const transcriptionSupported = useAppStore(
    (s) => s.appData?.transcriptionSupported === true,
  );
  const support = {
    faceScoring: faceScoringSupported,
    transcription: transcriptionSupported,
  };

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
        <Button variant="ghost" onClick={() => setStep((step - 1) as 1 | 2 | 3)}>
          Back
        </Button>
      ) : null}
    </span>
  );

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-background p-6">
      <div className="w-[min(860px,calc(100vw-3rem))] rounded-2xl border border-border bg-surface p-7 shadow-xl">
        <h1 className="text-xl font-semibold tracking-tight text-ink-strong">
          {reconfigure ? "Reconfigure" : "Setup"}
        </h1>
        <p className="mt-1 mb-6 text-sm text-ink-muted">
          {`Step ${step} of ${WIZARD_STEPS}`}
        </p>
        {error !== null ? (
          <OperationResult level="error" className="mb-4 text-sm">
            {error}
          </OperationResult>
        ) : null}

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
              {dirs.length === 0 ? (
                <li className="text-sm text-ink-muted">
                  Add at least one directory to continue.
                </li>
              ) : null}
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
              aria-invalid={
                (!timezonePending && !timezoneValid) || undefined
              }
              aria-describedby={
                !timezonePending && !timezoneValid ? timezoneErrorId : undefined
              }
              value={timezone}
              onChange={(e) => void setTimezone(e.target.value)}
            />
            <p
              id={timezoneErrorId}
              role={!timezonePending && !timezoneValid ? "alert" : undefined}
              className="mb-6 min-h-4 text-xs text-danger"
            >
              {timezonePending
                ? "Checking timezone…"
                : timezoneValid
                  ? ""
                  : "Error: Not a recognized timezone name"}
            </p>
            <div className="flex items-center justify-between">
              {leading}
              <Button
                variant="primary"
                disabled={timezonePending || !timezoneValid || timezone.trim() === ""}
                onClick={() => setStep(3)}
              >
                Next
              </Button>
            </div>
          </section>
        ) : null}

        {step === 3 ? (
          <section>
            <h2 className="mb-1 text-sm font-semibold text-ink-strong">
              OneCopy always prepares
            </h2>
            <p className="mb-2 text-sm text-ink-muted">
              OneCopy checks file identity, dates, companions, and live folder changes. It
              also prepares thumbnails, image previews, video posters, and supported file
              presentation. These are required for the library to work and have no off switch.
            </p>
            <h2 className="mb-1 mt-5 text-sm font-semibold text-ink-strong">
              Additional features
            </h2>
            <p className="mb-2 text-sm text-ink-muted">
              These can use substantial processing time. Change them now or later in Settings.
            </p>
            {(
              [
                ["videoSnapshotsEnabled", "Video scene snapshots"],
                ["similarPhotoAnalysisEnabled", "Similar-photo analysis"],
                ["scoreFaces", "Face scoring"],
                ["videoTranscriptionEnabled", "Video transcription"],
                ["audioTranscriptionEnabled", "Audio transcription"],
              ] as const satisfies readonly [OptionalFeatureId, string][]
            ).map(([id, label]) => (
              <Row key={id} label={label} hint={optionalFeatureReasons[id]}>
                <Toggle
                  checked={optionalFeatures[id]}
                  disabled={!optionalFeatureSupported(id, support)}
                  onChange={(enabled) => setOptionalFeature(id, enabled)}
                />
              </Row>
            ))}
            <div className="mt-6 flex items-center justify-between">
              {leading}
              <Button variant="primary" onClick={() => void finishWizard()}>
                Finish and scan
              </Button>
            </div>
          </section>
        ) : null}

      </div>
    </div>
  );
}
