// Main-window startup is an application workflow: load each store through its
// own adapter, then project the loaded config into the wizard and destination
// stores. No store imports a peer to make startup happen.

import { useAppStore } from "../state/app-store";
import { useBinariesStore } from "../state/binaries-store";
import { useDestinationsStore } from "../state/destinations-store";
import { useIssuesStore } from "../state/issues-store";
import { useSectionsStore } from "../state/sections-store";
import { useWizardStore } from "../state/wizard-store";
import { installScanEventWiring } from "./scan-events";
import { installItemWorkflow } from "./items";
import { installPreviewPersistence } from "./preview";
import { installComparisonEventWiring } from "./comparison";

export async function bootstrapApplication(): Promise<void> {
  installItemWorkflow();
  installPreviewPersistence();
  await Promise.all([
    installScanEventWiring(),
    installComparisonEventWiring(),
  ]);
  const [data] = await Promise.all([
    useAppStore.getState().reload(),
    useSectionsStore.getState().loadCounts(),
    useIssuesStore.getState().load(),
    useBinariesStore.getState().load(),
  ]);
  if (data === null) return;
  await useWizardStore.getState().init(data.config);
  useDestinationsStore.getState().init(data.config);
}
