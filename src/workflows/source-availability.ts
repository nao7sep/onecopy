import { useWizardStore } from "../state/wizard-store";
import { useSectionsStore } from "../state/sections-store";

let refreshInFlight: Promise<boolean> | null = null;
let refreshAgain = false;
let refreshQuiet = true;
let recoveryInFlight: Promise<void> | null = null;

/** Completion events can arrive during a volume probe. Keep one read in flight
 * and one owed refresh so the final notice reflects the latest completion. */
export function refreshSourceAvailability(quiet = true): Promise<boolean> {
  if (refreshInFlight !== null) {
    refreshAgain = true;
    refreshQuiet = refreshQuiet && quiet;
    return refreshInFlight;
  }
  refreshQuiet = quiet;
  refreshInFlight = (async () => {
    let accepted: boolean;
    do {
      refreshAgain = false;
      accepted = await useWizardStore.getState().recheckPresence(refreshQuiet);
    } while (refreshAgain);
    return accepted;
  })().finally(() => { refreshInFlight = null; });
  return refreshInFlight;
}

/** Check again owns one complete recovery request. The source-check worker
 * already restores watchers after its pass; never create a second owner here. */
export function recheckSources(): Promise<void> {
  if (recoveryInFlight !== null) return recoveryInFlight;
  recoveryInFlight = (async () => {
    if (!await refreshSourceAvailability(false)) return;
    const status = useWizardStore.getState();
    if (status.presenceUnknown || status.substitutedDirs.length > 0 || status.open) return;
    await useSectionsStore.getState().startSourceCheck("explicit");
  })().finally(() => { recoveryInFlight = null; });
  return recoveryInFlight;
}
