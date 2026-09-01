// Main-window startup is an application workflow: load each store through its
// own adapter, then project the loaded config into the wizard and destination
// stores. No store imports a peer to make startup happen.

import { useAppStore } from "../state/app-store";
import { useBinariesStore } from "../state/binaries-store";
import { useDestinationsStore } from "../state/destinations-store";
import { useIssuesStore } from "../state/issues-store";
import { useSectionsStore } from "../state/sections-store";
import { useWizardStore } from "../state/wizard-store";
import { stringArrayField } from "../utils/configProjection";
import { installScanEventWiring } from "./scan-events";
import { installItemWorkflow } from "./items";
import { installPreviewCommandWiring, installPreviewPersistence } from "./preview";
import { installComparisonEventWiring } from "./comparison";
import { installMutationEventWiring } from "./mutation-events";
import { installViewerWorkflow } from "./quick-view";
import { installPlaybackWorkflow } from "./playback";
import { installContentSessionWorkflow } from "./content-session";
import { installIssuesEventWiring } from "./issues";

export async function bootstrapApplication(): Promise<void> {
  installItemWorkflow();
  installPreviewPersistence();
  await Promise.all([
    installScanEventWiring(),
    installComparisonEventWiring(),
    installMutationEventWiring(),
    installViewerWorkflow(),
    installPreviewCommandWiring(),
    installPlaybackWorkflow(),
    installContentSessionWorkflow(),
    installIssuesEventWiring(),
  ]);
  const [data] = await Promise.all([
    useAppStore.getState().initialize(),
    useSectionsStore.getState().loadCounts(),
    useIssuesStore.getState().load(),
    useBinariesStore.getState().load(),
  ]);
  if (data === null) return;
  await useWizardStore.getState().init(data.config);
  useDestinationsStore.getState().init(data.config);
  const wizard = useWizardStore.getState();
  const sources = stringArrayField(data.config, "sourceDirs");
  const checkAfterLaunch = data.config?.checkSourceFoldersAtLaunch !== false;
  let sourceCheckStarted = false;
  if (
    checkAfterLaunch &&
    sources.length > 0 &&
    !wizard.open &&
    wizard.missingDirs.length === 0 &&
    wizard.substitutedDirs.length === 0
  ) {
    // Event wiring, initial data, the first section projection, and source
    // presence are settled before this finite background pass starts. The
    // main interface is therefore usable and cannot miss its early state.
    sourceCheckStarted = await useSectionsStore.getState().startSourceCheck();
  }
  if (!sourceCheckStarted) {
    // The source pass wakes this independent tail at its terminal boundary.
    // Without that pass, Main explicitly admits the tail after event wiring
    // and the first usable projection are ready.
    await useSectionsStore.getState().admitBackgroundCompletion();
  }
}
