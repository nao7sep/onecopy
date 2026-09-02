// The complete Settings Save transaction. The settings store owns its draft;
// this application edge publishes durable configuration and playback view
// state through their separate owners, then refreshes affected projections.

import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import { useItemsStore } from "../state/items-store";
import { useSectionsStore } from "../state/sections-store";
import { useSettingsStore } from "../state/settings-store";
import { useWizardStore } from "../state/wizard-store";
import { recordActionFailure } from "../state/notifications-store";

export async function saveSettings(): Promise<void> {
  const { draft, opened, timezoneValid, timezonePending } = useSettingsStore.getState();
  if (!draft || !timezoneValid || timezonePending) return;
  const sourceDirsChanged =
    opened !== null && JSON.stringify(draft.sourceDirs) !== JSON.stringify(opened.sourceDirs);
  const { soundEnabled, playbackVolume, ...configDraft } = draft;
  useSettingsStore.setState({ saving: true, message: "", messageLevel: null });
  // Config publication is the Save transaction's commit point. Index
  // projection is durable follow-up work: once publication succeeds, close
  // the draft surface rather than leaving it looking unsaved for the duration
  // of a million-row rebuild or after a cancellable repair.
  try {
    await useAppStore.getState().patchConfig(configDraft, { reportFailure: false });
    await useAppStore.getState().patchState({ soundEnabled, playbackVolume });
    useSettingsStore.setState({
      open: false,
      draft: null,
      opened: null,
      saving: false,
    });
  } catch (error) {
    useSettingsStore.setState({
      saving: false,
      message: "Settings could not be saved. Your changes are still here; try again.",
      messageLevel: "error",
    });
    log.error("settings save failed", toErrorFields(error));
    recordActionFailure("settings-save-failed", "Couldn’t save Settings.", error);
    return;
  }

  let resolved: number | null = null;
  try {
    resolved = await invoke<number>("re_resolve_all");
  } catch (error) {
    useItemsStore.setState({
      message: "Settings were saved, but the library could not be updated. Try refreshing the section.",
    });
    log.error("settings re-index failed after save", toErrorFields(error));
    recordActionFailure(
      "settings-reindex-failed",
      "Settings were saved, but OneCopy couldn’t update the library.",
      error,
    );
  }
  try {
    await Promise.all([
      useSectionsStore.getState().loadCounts(),
      useItemsStore.getState().refresh(),
      useWizardStore.getState().recheckPresence(),
    ]);
  } catch (error) {
    log.error("settings projections refresh failed", toErrorFields(error));
    useItemsStore.setState({
      message: "Settings were saved, but OneCopy couldn’t refresh the interface.",
    });
    recordActionFailure(
      "settings-refresh-failed",
      "Settings were saved, but OneCopy couldn’t refresh the interface.",
      error,
    );
  }
  if (sourceDirsChanged) {
    try {
      await useSectionsStore.getState().startSourceCheck();
    } catch (error) {
      log.error("source-folder check failed to start after settings save", toErrorFields(error));
      recordActionFailure(
        "settings-source-check-failed",
        "Settings were saved, but OneCopy couldn’t start checking source folders.",
        error,
      );
    }
  }
  log.info("settings saved", { resolved });
}
