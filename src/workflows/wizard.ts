// The complete Setup Finish transaction. Wizard state stays local to its
// store; persistence and the resulting scan are coordinated at this edge.

import { log, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import { useSectionsStore } from "../state/sections-store";
import { useWizardStore } from "../state/wizard-store";

interface WizardSubmission {
  dirs: { path: string }[];
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
  try {
    await useAppStore.getState().patchConfig({
      sourceDirs: submission.dirs.map((dir) => dir.path),
      defaultTimezone: submission.timezone,
      ...submission.optionalFeatures,
    });
    // Rechecking after persistence prunes trust for removed roots; a later
    // re-add is first sight rather than a false substitution.
    await useWizardStore.getState().recheckPresence();
    if (stillOwnsSubmission(submission)) {
      useWizardStore.setState({ open: false, finishing: false });
    } else if (useWizardStore.getState().open) {
      useWizardStore.setState({
        finishing: false,
        error:
          "Setup was saved, but newer changes are still open. Review them, then finish again.",
      });
    }
    log.info("wizard finished", { sourceDirs: submission.dirs.length });
    await useSectionsStore.getState().startSourceCheck();
  } catch (error) {
    log.error("wizard save failed", toErrorFields(error));
    if (useWizardStore.getState().finishing) {
      useWizardStore.setState({
        finishing: false,
        error: "Setup could not be saved. Your changes are still here; try again.",
      });
    }
  }
}

export function finishWizard(): Promise<void> {
  if (finishInFlight !== null) return finishInFlight;
  const { dirs, timezone, timezoneValid, timezonePending, optionalFeatures } =
    useWizardStore.getState();
  if (!timezoneValid || timezonePending || timezone.trim() === "") {
    return Promise.resolve();
  }
  const submission: WizardSubmission = {
    dirs: dirs.map((dir) => ({ path: dir.path })),
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
