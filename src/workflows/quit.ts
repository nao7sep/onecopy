// One renderer save attempt for every application quit. The native owner
// retains the process and handles an OS takeover independently of this page.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { flushConfigForShutdown, flushStatePatchesForShutdown, reportStatePatchFailure, resumeStatePatchesAfterFailedShutdown } from "../state/app-store";
import { settingsDraftIsDirty, useSettingsStore } from "../state/settings-store";
import { useAppShellStore } from "../state/app-shell-store";
import { reportWindowCall } from "../repositories";

export type QuitChoice = "retry" | "cancel" | "quit" | "session";
export const useQuitSaveStore = create<{ choose: ((choice: QuitChoice) => void) | null; presented: (() => void) | null }>(() => ({ choose: null, presented: null }));
/** An ordinary quit over unsaved Settings edits asks first (developer
 * decision): true discards them and quits, false keeps editing. */
export const useQuitDiscardStore = create<{ choose: ((discard: boolean) => void) | null; presented: (() => void) | null }>(() => ({ choose: null, presented: null }));
let attempt: Promise<void> | null = null;
let requiredSave: Promise<void> | null = null;
let sessionEnding = false;
let sessionRevision = 0;

function bounded<T>(work: Promise<T>, milliseconds: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("quit save exceeded its deadline")), milliseconds);
    work.then(resolve, reject).finally(() => clearTimeout(timer));
  });
}

// Every queued settings write settles first, a Settings save in flight
// included, and pending sound and volume are written.
function saveRequired(): Promise<void> {
  if (requiredSave === null) {
    requiredSave = flushConfigForShutdown().finally(() => { requiredSave = null; });
  }
  return bounded(requiredSave, 1500);
}

/** Whether the quit may go on past the Settings draft: true when there are no
 * unsaved edits or the user chose Discard. Checked after the required save, so
 * a Settings save that was in flight has settled. */
async function settingsMayBeDiscarded(): Promise<boolean> {
  if (!settingsDraftIsDirty(useSettingsStore.getState())) return true;
  let markPresented!: () => void;
  const presented = new Promise<void>((resolve) => { markPresented = resolve; });
  const decision = new Promise<boolean>((resolve) => {
    useQuitDiscardStore.setState({ presented: markPresented, choose: (discard) => {
      markPresented();
      useQuitDiscardStore.setState({ choose: null, presented: null });
      resolve(discard);
    } });
  });
  // Like the save decision: a prompt that cannot be shown keeps editing.
  await bounded(presented, 500).catch(() => useQuitDiscardStore.getState().choose?.(false));
  if (!(await decision)) return false;
  useSettingsStore.getState().discardDraft();
  useAppShellStore.getState().closeUtility();
  return true;
}

async function decide(): Promise<QuitChoice> {
  let markPresented!: () => void;
  const presented = new Promise<void>((resolve) => { markPresented = resolve; });
  const decision = new Promise<QuitChoice>((resolve) => {
    useQuitSaveStore.setState({ presented: markPresented, choose: (choice) => {
      markPresented();
      useQuitSaveStore.setState({ choose: null, presented: null });
      resolve(choice);
    } });
  });
  // A failed renderer may have lost its modal host while the module and
  // pending settings survive. Missing presentation cancels, never discards.
  await bounded(presented, 500).catch(() => useQuitSaveStore.getState().choose?.("cancel"));
  return decision;
}

export function requestQuit(): Promise<void> {
  if (attempt !== null) return attempt;
  if (sessionEnding) return Promise.resolve();
  const revision = sessionRevision;
  attempt = (async () => {
    while (!sessionEnding && revision === sessionRevision) {
      try {
        // Required settings always get their attempt, irrespective of optional
        // screen-state failure. Optional work cannot delay the required save.
        await saveRequired();
        break;
      } catch (error) {
        reportStatePatchFailure(error);
        if (sessionEnding || revision !== sessionRevision) return;
        const choice = await decide();
        if (choice === "session") return;
        if (choice === "cancel") { resumeStatePatchesAfterFailedShutdown(); return; }
        if (choice === "quit") break;
      }
    }
    if (sessionEnding || revision !== sessionRevision) return;
    if (!(await settingsMayBeDiscarded())) return;
    if (sessionEnding || revision !== sessionRevision) return;
    await bounded(flushStatePatchesForShutdown(), 500).catch(reportStatePatchFailure);
    if (!sessionEnding && revision === sessionRevision) await invoke("request_app_exit");
  })().catch((error) => {
    resumeStatePatchesAfterFailedShutdown();
    reportWindowCall("request application exit")(error);
  }).finally(() => { attempt = null; });
  return attempt;
}

export async function endSession(id = 0): Promise<void> {
  sessionRevision += 1;
  sessionEnding = true;
  useQuitSaveStore.getState().choose?.("session");
  // The OS ending the session never asks; the draft is left as it is.
  useQuitDiscardStore.getState().choose?.(false);
  try { await saveRequired(); } catch (error) { reportStatePatchFailure(error); }
  finally { await invoke("session_end_saved", { id }).catch(reportWindowCall("report session-end save")); }
}

export function cancelQuitDecision(): void {
  useQuitSaveStore.getState().choose?.("cancel");
}

export function cancelSessionEnd(): void {
  sessionEnding = false;
  sessionRevision += 1;
  resumeStatePatchesAfterFailedShutdown();
}

export async function installQuitWorkflow(): Promise<void> {
  await listen("app://quit-request", () => { void requestQuit(); });
  await listen<number>("app://session-ending", (event) => { void endSession(event.payload); });
  await listen("app://session-end-cancelled", cancelSessionEnd);
}
