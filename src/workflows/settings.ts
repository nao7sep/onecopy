// The complete Settings Save transaction. The settings store owns its draft;
// this application edge coordinates the config owner and the projections that
// must be refreshed after resolver settings change.

import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import { useItemsStore } from "../state/items-store";
import { useSectionsStore } from "../state/sections-store";
import { useSettingsStore } from "../state/settings-store";
import { useWizardStore } from "../state/wizard-store";

export async function saveSettings(): Promise<void> {
  const { draft, timezoneValid, timezonePending } = useSettingsStore.getState();
  if (!draft || !timezoneValid || timezonePending) return;
  useSettingsStore.setState({ saving: true, message: "" });
  try {
    // Patch exactly the draft's keys. Destination roots and other config
    // owned by separate surfaces remain untouched.
    await useAppStore.getState().patchConfig({ ...draft });
    // Re-resolve from stored evidence, then refresh every affected projection.
    const resolved = await invoke<number>("re_resolve_all");
    await Promise.all([
      useSectionsStore.getState().loadCounts(),
      useItemsStore.getState().refresh(),
      useWizardStore.getState().recheckPresence(),
    ]);
    useSettingsStore.setState({
      open: false,
      draft: null,
      opened: null,
      saving: false,
    });
    log.info("settings saved", { resolved });
  } catch (error) {
    useSettingsStore.setState({ saving: false, message: String(error) });
    log.error("settings save failed", toErrorFields(error));
  }
}
