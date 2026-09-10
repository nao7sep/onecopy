// Main-window startup is an application workflow: load each store through its
// own adapter, then project the loaded config into the wizard and destination
// stores. No store imports a peer to make startup happen.

import { useAppStore } from "../state/app-store";
import {
  installBinariesEventWiring,
  useBinariesStore,
} from "../state/binaries-store";
import { useDestinationsStore } from "../state/destinations-store";
import { useIssuesStore } from "../state/issues-store";
import { useSectionsStore } from "../state/sections-store";
import { useWizardStore } from "../state/wizard-store";
import { stringArrayField } from "../utils/configProjection";
import { installScanEventWiring } from "./scan-events";
import { installItemWorkflow } from "./items";
import { installWorkAttention } from "./work-attention";
import { installPreviewCommandWiring, installPreviewPersistence } from "./preview";
import { installComparisonEventWiring } from "./comparison";
import { installMutationEventWiring } from "./mutation-events";
import { installViewerWorkflow } from "./quick-view";
import { installPlaybackWorkflow } from "./playback";
import { installContentSessionWorkflow } from "./content-session";
import { installIssuesEventWiring } from "./issues";
import { installDerivedWorkEventWiring } from "../state/derived-work-store";
import { installTranscriptEventWiring } from "../state/transcript-store";
import type { LoadedAppData } from "../repositories";
import { startAutomaticReleaseCheck } from "../state/release-check-store";

let completedData: LoadedAppData | null = null;
let bootstrapInFlight: Promise<void> | null = null;

/** One owner for the main-window bootstrap. React development remounts and
 * renderer recovery must join the same admission, not duplicate listeners or
 * initial queries against the same loaded application data. */
export function bootstrapApplication(): Promise<void> {
  const current = useAppStore.getState().appData;
  if (current !== null && current === completedData) return Promise.resolve();
  if (bootstrapInFlight !== null) return bootstrapInFlight;
  bootstrapInFlight = bootstrapOnce().finally(() => {
    bootstrapInFlight = null;
  });
  return bootstrapInFlight;
}

async function bootstrapOnce(): Promise<void> {
  // The backend's immutable startup gate is the admission boundary for every
  // feature listener, query, and worker-triggering command in the main window.
  const data = await useAppStore.getState().initialize();
  if (data === null) return;

  installItemWorkflow();
  installWorkAttention();
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
    installDerivedWorkEventWiring(),
    installBinariesEventWiring(),
    installTranscriptEventWiring(),
  ]);
  await Promise.all([
    useSectionsStore.getState().loadCounts(),
    useIssuesStore.getState().load(),
    useBinariesStore.getState().load(),
  ]);
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
    wizard.substitutedDirs.length === 0
  ) {
    // Event wiring, initial data, the first section projection, and source
    // presence are settled before this finite background pass starts. The
    // main interface is therefore usable and cannot miss its early state.
    sourceCheckStarted = await useSectionsStore.getState().startSourceCheck("automatic");
  }
  if (!sourceCheckStarted) {
    // The source pass wakes this independent tail at its terminal boundary.
    // Without that pass, Main explicitly admits the tail after event wiring
    // and the first usable projection are ready.
    await useSectionsStore.getState().admitBackgroundCompletion();
  }
  completedData = data;
  // Optional metadata-only work begins only after Main is fully usable and is
  // never part of the bootstrap promise the shell waits on.
  void startAutomaticReleaseCheck(data);
}
