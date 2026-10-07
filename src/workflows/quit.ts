// One renderer save attempt for every application quit. The native owner
// retains the process and handles an OS takeover independently of this page.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { flushPlaybackConfigForShutdown } from "./playback";
import { flushStatePatchesForShutdown, reportStatePatchFailure, resumeStatePatchesAfterFailedShutdown } from "../state/app-store";
import { reportWindowCall } from "../repositories";

export type QuitChoice = "retry" | "cancel" | "quit" | "session";
export const useQuitSaveStore = create<{ choose: ((choice: QuitChoice) => void) | null; presented: (() => void) | null }>(() => ({ choose: null, presented: null }));
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

function saveRequired(): Promise<void> {
  if (requiredSave === null) {
    requiredSave = flushPlaybackConfigForShutdown().finally(() => { requiredSave = null; });
  }
  return bounded(requiredSave, 1500);
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
