import { useWizardStore } from "../state/wizard-store";
import { finishWizard } from "../workflows/wizard";
import { useBlockingSurface } from "../hooks/useBlockingSurface";
import DirectoryRow from "./DirectoryRow";
import Button from "./ui/Button";
import { timeZoneOptions } from "../utils/timezones";
import { Plus } from "lucide-react";
import { Row, Toggle } from "./ui/Field";
import type { OptionalFeatureId } from "../models/optionalFeatures";
import OperationResult from "./ui/OperationResult";
import { CATALOGUES } from "../i18n/catalogues";
import { useI18n } from "../i18n/I18nContext";
import { LANGUAGES, normalizeLanguagePreference } from "../i18n/languages";

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
  const { t, text } = useI18n();
  const step = useWizardStore((s) => s.step);
  const language = useWizardStore((s) => s.language);
  const setLanguage = useWizardStore((s) => s.setLanguage);
  const dirs = useWizardStore((s) => s.dirs);
  const timezone = useWizardStore((s) => s.timezone);
  const error = useWizardStore((s) => s.error);
  const finishing = useWizardStore((s) => s.finishing);
  const reconfigure = useWizardStore((s) => s.reconfigure);
  const optionalFeatures = useWizardStore((s) => s.optionalFeatures);
  const addDirs = useWizardStore((s) => s.addDirs);
  const removeDir = useWizardStore((s) => s.removeDir);
  const setStep = useWizardStore((s) => s.setStep);
  const setOptionalFeature = useWizardStore((s) => s.setOptionalFeature);
  const setTimezone = useWizardStore((s) => s.setTimezone);
  const cancel = useWizardStore((s) => s.cancel);

  useBlockingSurface();

  /** A re-run offers Cancel on EVERY page (developer, 2026-08-17 — being
   * three pages deep is no reason to walk back out first), beside Back on
   * steps 2+. A first run keeps neither on page 1: it is completable only,
   * with nothing behind it to return to. */
  const leading = (
    <span className="flex items-center gap-2">
      {reconfigure ? (
        <Button variant="ghost" disabled={finishing} onClick={cancel}>
          {t("common.cancel")}
        </Button>
      ) : null}
      {step > 1 ? (
        <Button variant="ghost" disabled={finishing} onClick={() => setStep((step - 1) as 1 | 2 | 3)}>
          {t("wizard.back")}
        </Button>
      ) : null}
    </span>
  );

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-background p-6">
      <div className="w-[min(860px,calc(100vw-3rem))] rounded-2xl border border-border bg-surface p-7 shadow-xl">
        <h1 className="text-xl font-semibold tracking-tight text-ink-strong">
          {reconfigure ? t("wizard.reconfigureTitle") : t("wizard.setupTitle")}
        </h1>
        <p className="mt-1 mb-6 text-sm text-ink-muted">
          {t("wizard.step", { step, total: WIZARD_STEPS })}
        </p>
        {error !== null ? (
          <OperationResult level="error" className="mb-4 text-sm">
            {text(error)}
          </OperationResult>
        ) : null}

        {step === 1 ? (
          <section>
            <h2 className="mb-1 text-sm font-semibold text-ink-strong">
              {t("wizard.language")}
            </h2>
            <select
              className="mb-6 rounded-md border border-input-border bg-surface px-2 py-1 text-sm text-ink"
              value={language}
              onChange={(e) => setLanguage(normalizeLanguagePreference(e.target.value))}
            >
              <option value="system">{t("settings.languageSystem")}</option>
              {LANGUAGES.map((tag) => (
                <option key={tag} value={tag} lang={tag}>
                  {CATALOGUES[tag]["language.name"] as string}
                </option>
              ))}
            </select>
            <h2 className="mb-1 text-sm font-semibold text-ink-strong">
              {t("wizard.directories")}
            </h2>
            <p className="mb-3 text-sm text-ink-muted">
              {t("wizard.directoriesHint")}
            </p>
            <ul className="mb-4 max-h-64 space-y-1.5 overflow-y-auto">
              {dirs.length === 0 ? (
                <li className="text-sm text-ink-muted">
                  {t("wizard.noDirectories")}
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
              {t("settings.addDirectory")}
            </Button>
            <div className="flex items-center justify-between">
              {leading}
              <Button variant="primary" disabled={dirs.length === 0} onClick={() => setStep(2)}>
                {t("wizard.next")}
              </Button>
            </div>
          </section>
        ) : null}

        {step === 2 ? (
          <section>
            <h2 className="mb-1 text-sm font-semibold text-ink-strong">
              {t("settings.defaultTimezone")}
            </h2>
            <p className="mb-2 text-sm text-ink-muted">
              {t("wizard.timezoneHint")}
            </p>
            <select
              className="mb-6 h-9 w-full rounded-lg border border-input-border bg-background px-3 text-sm text-ink outline-none transition-colors focus-visible:ring-2 focus-visible:ring-primary-ring"
              value={timezone}
              onChange={(e) => setTimezone(e.target.value)}
            >
              {timeZoneOptions(timezone).map((zone) => (
                <option key={zone} value={zone}>
                  {zone}
                </option>
              ))}
            </select>
            <div className="flex items-center justify-between">
              {leading}
              <Button
                variant="primary"
                disabled={timezone.trim() === ""}
                onClick={() => setStep(3)}
              >
                {t("wizard.next")}
              </Button>
            </div>
          </section>
        ) : null}

        {step === 3 ? (
          <section>
            <h2 className="mb-1 text-sm font-semibold text-ink-strong">
              {t("wizard.alwaysPrepares")}
            </h2>
            <p className="mb-2 text-sm text-ink-muted">
              {t("wizard.alwaysPreparesHint")}
            </p>
            <h2 className="mb-1 mt-5 text-sm font-semibold text-ink-strong">
              {t("wizard.additionalFeatures")}
            </h2>
            <p className="mb-2 text-sm text-ink-muted">
              {t("wizard.additionalFeaturesHint")}
            </p>
            {(
              [
                ["videoSnapshotsEnabled", "wizard.videoSnapshots"],
                ["similarPhotoAnalysisEnabled", "wizard.similarPhotoAnalysis"],
                ["scoreFaces", "wizard.faceScoring"],
                ["videoTranscriptionEnabled", "wizard.videoTranscription"],
                ["audioTranscriptionEnabled", "wizard.audioTranscription"],
              ] as const satisfies readonly [OptionalFeatureId, string][]
            ).map(([id, label]) => (
              <Row key={id} label={t(label)}>
                <Toggle
                  checked={optionalFeatures[id]}
                  disabled={finishing}
                  onChange={(enabled) => setOptionalFeature(id, enabled)}
                />
              </Row>
            ))}
            <div className="mt-6 flex items-center justify-between">
              {leading}
              <Button
                variant="primary"
                disabled={finishing}
                onClick={() => void finishWizard()}
              >
                {finishing ? t("wizard.finishing") : t("wizard.finish")}
              </Button>
            </div>
          </section>
        ) : null}

      </div>
    </div>
  );
}
