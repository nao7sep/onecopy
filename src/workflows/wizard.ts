// The complete Setup Finish transaction. Wizard state stays local to its
// store; persistence and the resulting scan are coordinated at this edge.

import { log, toErrorFields } from "../repositories";
import type { LanguagePreference } from "../i18n/languages";
import { message } from "../i18n/translate";
import { useAppStore } from "../state/app-store";
import { recordActionFailure } from "../state/notifications-store";
import { useSectionsStore } from "../state/sections-store";
import { useWizardStore } from "../state/wizard-store";

interface WizardSubmission {
  dirs: { path: string }[];
  language: LanguagePreference;
  timezone: string;
  optionalFeatures: ReturnType<typeof useWizardStore.getState>["optionalFeatures"];
}

let finishInFlight: Promise<void> | null = null;

function stillOwnsSubmission(submission: WizardSubmission): boolean {
  const current = useWizardStore.getState();
  return (
    current.open &&
    current.finishing &&
    JSON.stringify(current.dirs.map((dir) => dir.path)) ===
      JSON.stringify(submission.dirs.map((dir) => dir.path)) &&
    current.timezone === submission.timezone &&
    JSON.stringify(current.optionalFeatures) ===
      JSON.stringify(submission.optionalFeatures)
  );
}

async function finishSubmission(submission: WizardSubmission): Promise<void> {
  // Config publication is the transaction's commit point, and the open form is
  // where its failure belongs: the core stays quiet so one failed write is one
  // notice and one Issue, and the follow-up work below keeps its own boundary.
  try {
    await useAppStore.getState().patchConfig(
      {
        sourceDirs: submission.dirs.map((dir) => dir.path),
        language: submission.language,
        defaultTimezone: submission.timezone,
        ...submission.optionalFeatures,
      },
      { reportFailure: false },
    );
  } catch (error) {
    log.error("wizard save failed", toErrorFields(error));
    recordActionFailure("wizard-save-failed", message("wizard.saveFailed"), error);
    if (useWizardStore.getState().finishing) {
      useWizardStore.setState({
        finishing: false,
        error: message("wizard.saveFailed"),
      });
    }
    return;
  }
  try {
    // Rechecking after persistence prunes trust for removed roots; a later
    // re-add is first sight rather than a false substitution.
    await useWizardStore.getState().recheckPresence();
    if (stillOwnsSubmission(submission)) {
      useWizardStore.setState({ open: false, finishing: false });
    } else if (useWizardStore.getState().open) {
      useWizardStore.setState({
        finishing: false,
        error: message("wizard.savedButStale"),
      });
    }
    log.info("wizard finished", { sourceDirs: submission.dirs.length });
    await useSectionsStore.getState().startSourceCheck("automatic");
  } catch (error) {
    log.error("wizard follow-up after save failed", toErrorFields(error));
    if (useWizardStore.getState().finishing) {
      useWizardStore.setState({
        finishing: false,
        error: message("wizard.saveFailed"),
      });
    }
  }
}

export function finishWizard(): Promise<void> {
  if (finishInFlight !== null) return finishInFlight;
  const { dirs, language, timezone, optionalFeatures } =
    useWizardStore.getState();
  if (timezone.trim() === "") {
    return Promise.resolve();
  }
  const submission: WizardSubmission = {
    dirs: dirs.map((dir) => ({ path: dir.path })),
    language,
    timezone,
    optionalFeatures: { ...optionalFeatures },
  };
  useWizardStore.setState({ finishing: true, error: null });
  const operation = finishSubmission(submission).finally(() => {
    if (useWizardStore.getState().finishing) {
      useWizardStore.setState({ finishing: false });
    }
    if (finishInFlight === operation) finishInFlight = null;
  });
  finishInFlight = operation;
  return operation;
}
