// The complete Settings Save transaction. The settings store owns its draft;
// this application edge publishes durable configuration, then refreshes
// affected projections.

import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import { useItemsStore } from "../state/items-store";
import { useSectionsStore } from "../state/sections-store";
import { changedSettingsSets, useSettingsStore } from "../state/settings-store";
import { useWizardStore } from "../state/wizard-store";
import { message } from "../i18n/translate";
import {
  recordActionFailure,
  reportActionFailure,
  reportInfoNotice,
} from "../state/notifications-store";
import { newActivityOperationId, recordActivity } from "../repositories/activity";
import { useAppShellStore } from "../state/app-shell-store";
import { refreshBackgroundWorkSoon } from "../state/derived-work-store";
import { useDestinationsStore } from "../state/destinations-store";
import { reconcileComparisonMembership } from "./comparison";

type LibrarySettingsOutcome = { status: "applied"; resolved: number } | { status: "owed" };

export async function saveSettings(): Promise<void> {
  const { draft, opened } = useSettingsStore.getState();
  if (!draft) return;
  const sourceDirsChanged =
    opened !== null && JSON.stringify(draft.sourceDirs) !== JSON.stringify(opened.sourceDirs);
  const visibilityChanged = opened === null || ["ignoredFileNames", "hideDotNames", "hideHiddenAttributes", "hideSystemAttributes"]
    .some((key) => JSON.stringify(draft[key as keyof typeof draft]) !== JSON.stringify(opened[key as keyof typeof opened]));
  useSettingsStore.setState({ saving: true, message: null, messageLevel: null });
  const operationId = newActivityOperationId("settings");
  recordActivity({
    kind: "started",
    owner: "settings",
    operationId,
    current: "running",
    reason: "user",
  });
  // Config publication is the Save transaction's commit point; sound and
  // volume are settings like the rest and ride in the same save.
  try {
    await useAppStore.getState().saveConfig(changedSettingsSets(draft, opened), { reportFailure: false });
  } catch (error) {
    useSettingsStore.setState({
      saving: false,
      message: message("settings.saveFailed"),
      messageLevel: "error",
    });
    log.error("settings save failed", toErrorFields(error));
    recordActionFailure("settings-save-failed", message("settings.saveFailedNotice"), error);
    recordActivity({
      kind: "failed",
      owner: "settings",
      operationId,
      previous: "running",
      current: "failed",
      reason: "error",
    });
    return;
  }

  let followUpFailed = false;
  // Index projection is durable follow-up work: once the config publishes,
  // close the draft rather than holding Settings open for a potentially large
  // rebuild.
  useSettingsStore.setState({
    draft: null,
    opened: null,
    saving: false,
  });
  useAppShellStore.getState().closeUtility();

  // The backend compares the saved settings with those the index was
  // projected with. When it cannot be admitted in time the apply stays owed
  // and the file-information owner performs it at its next turn.
  let resolved: number | null = null;
  try {
    const outcome = await invoke<LibrarySettingsOutcome>("apply_library_settings");
    if (outcome.status === "applied") {
      resolved = outcome.resolved;
    } else {
      reportInfoNotice("settings-apply-owed", message("settings.applyOwedNotice"));
    }
  } catch (error) {
    followUpFailed = true;
    log.error("settings re-index failed after save", toErrorFields(error));
    reportActionFailure(
      "settings-reindex-failed",
      message("settings.reindexFailedNotice"),
      error,
    );
  }
  try {
    if (visibilityChanged) await useDestinationsStore.getState().reconcileVisibility();
    await Promise.all([
      useSectionsStore.getState().loadCounts(),
      useItemsStore.getState().refresh(),
      useWizardStore.getState().recheckPresence(),
    ]);
    if (visibilityChanged) await reconcileComparisonMembership();
  } catch (error) {
    followUpFailed = true;
    log.error("settings projections refresh failed", toErrorFields(error));
    reportActionFailure(
      "settings-refresh-failed",
      message("settings.refreshFailedNotice"),
      error,
    );
  }
  if (sourceDirsChanged) {
    try {
      await useSectionsStore.getState().startSourceCheck("automatic");
    } catch (error) {
      followUpFailed = true;
      log.error("source-folder check failed to start after settings save", toErrorFields(error));
      recordActionFailure(
        "settings-source-check-failed",
        message("settings.sourceCheckFailedNotice"),
        error,
      );
    }
  }
  const failed = followUpFailed;
  log.info(failed ? "settings partially saved" : "settings saved", {
    resolved,
  });
  refreshBackgroundWorkSoon();
  recordActivity({
    kind: failed ? "failed" : "completed",
    owner: "settings",
    operationId,
    previous: "running",
    current: failed ? "failed" : "succeeded",
    reason: failed ? "error" : "completion",
  });
}
