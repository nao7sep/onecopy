// The complete Setup Finish transaction. Wizard state stays local to its
// store; persistence and the resulting scan are coordinated at this edge.

import { log, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import { useSectionsStore } from "../state/sections-store";
import { useWizardStore } from "../state/wizard-store";

export async function finishWizard(): Promise<void> {
  const { dirs, timezone, timezoneValid, timezonePending } =
    useWizardStore.getState();
  if (!timezoneValid || timezonePending || timezone.trim() === "") return;
  try {
    await useAppStore.getState().patchConfig({
      sourceDirs: dirs.map((dir) => dir.path),
      defaultTimezone: timezone,
    });
    // Rechecking after persistence prunes trust for removed roots; a later
    // re-add is first sight rather than a false substitution.
    await useWizardStore.getState().recheckPresence();
    useWizardStore.setState({ open: false });
    log.info("wizard finished", { sourceDirs: dirs.length });
    await useSectionsStore.getState().startScan();
  } catch (error) {
    log.error("wizard save failed", toErrorFields(error));
  }
}
