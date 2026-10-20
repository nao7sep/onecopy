// One renderer save attempt for every application quit. The native owner
// retains the process and handles an OS takeover independently of this page.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { create } from "zustand";
import { flushConfigForShutdown, flushStatePatchesForShutdown, reportStatePatchFailure, resumeStatePatchesAfterFailedShutdown } from "../state/app-store";
import { settingsDraftIsDirty, useSettingsStore } from "../state/settings-store";
import { useAppShellStore } from "../state/app-shell-store";
import { useMutationStore } from "../state/mutation-store";
import { useBinariesStore } from "../state/binaries-store";
import { useFullscreenViewStore } from "../state/fullscreen-view-store";
import { closeFullscreenView } from "./fullscreen-view";
import { focusWhileActive, reportWindowCall } from "../repositories";
import { useComparisonStore } from "../state/comparison-store";
import { documentTranslator } from "../i18n/I18nContext";
import type { MessageKey } from "../i18n/catalogues";
import { message, type Message } from "../i18n/translate";
import type { MutationKind } from "../models/mutation";
import { toolName } from "../models/coreLabels";

/** One quit question. In Main its host draws it and acknowledges that through
 * `presented`; a standalone question is a native dialog and has no host. */
export interface QuitQuestion<T> {
  choose: ((choice: T) => void) | null;
  presented: (() => void) | null;
  standalone: boolean;
}

/** What `ask` needs of a question's store. */
interface QuestionOwner<T> {
  getState: () => QuitQuestion<T>;
  setState: (partial: Partial<QuitQuestion<T>>) => void;
}

function questionStore<T>() {
  return create<QuitQuestion<T>>(() => ({ choose: null, presented: null, standalone: false }));
}

export type QuitChoice = "retry" | "cancel" | "quit" | "session";
export const useQuitSaveStore = questionStore<QuitChoice>();
/** An ordinary quit over unsaved Settings edits asks first (developer
 * decision): true discards them and quits, false keeps editing. */
export const useQuitDiscardStore = questionStore<boolean>();
/** An ordinary quit over a running file operation or managed-tool install
 * asks first: true stops it and quits, false keeps it running. `body` names
 * the running work for the host. */
export const useQuitRunningWorkStore = create<QuitQuestion<boolean> & { body: Message | null }>(
  () => ({ choose: null, presented: null, standalone: false, body: null }),
);
let attempt: Promise<void> | null = null;
let requiredSave: Promise<void> | null = null;
let sessionEnding = false;
let sessionRevision = 0;

function bounded<T>(work: Promise<T>, milliseconds: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("quit step exceeded its deadline")), milliseconds);
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

/** Whether Main is on screen, so a question there can be seen. A minimized or
 * hidden Main is never brought back to ask (window-conventions): the question
 * stands alone instead, even while Records or Preview shows, since the
 * questions' only in-app host is Main. An unreadable state stands alone too;
 * that dialog is usable whatever Main's state. */
async function mainInView(): Promise<boolean> {
  try {
    const main = getCurrentWindow();
    const [minimized, visible] = await Promise.all([main.isMinimized(), main.isVisible()]);
    return !minimized && visible;
  } catch {
    return false;
  }
}

/** The standalone form of a question: its choices in button order, the safe
 * one first, which the native dialog makes its default. */
interface StandaloneQuestion<T> {
  title: Message;
  body: Message;
  choices: readonly (readonly [T, MessageKey])[];
}

/** Asks one question and settles with the user's choice, or `safe` when it
 * cannot be shown or the OS ending the session overtook it. */
async function ask<T>(store: QuestionOwner<T>, revision: number, safe: T, standalone: StandaloneQuestion<T>): Promise<T> {
  const inMain = await mainInView();
  if (sessionEnding || revision !== sessionRevision) return safe;
  if (inMain && useFullscreenViewStore.getState().session !== null) {
    // The fullscreen view covers Main on its display, so a question in Main
    // would be hidden. It closes the way Escape closes it, returning Main
    // with the item selected; a quit with nothing to ask leaves it alone.
    await bounded(closeFullscreenView(), 500).catch(reportWindowCall("close the fullscreen view for a quit question"));
    if (sessionEnding || revision !== sessionRevision) return safe;
  }
  if (inMain && useComparisonStore.getState().open) {
    // Comparison is drawn inside Main, beneath its modals, and holds its keys
    // while one is open; a quit from another display's Comparison window
    // moves the keyboard to Main so the question can be answered there.
    await focusWhileActive("main").catch(reportWindowCall("focus Main for a quit question"));
  }
  let markPresented!: () => void;
  const presented = new Promise<void>((resolve) => { markPresented = resolve; });
  let choose!: (choice: T) => void;
  const decision = new Promise<T>((resolve) => {
    let settled = false;
    choose = (choice) => {
      // A late native answer after the OS took over changes nothing.
      if (settled) return;
      settled = true;
      markPresented();
      store.setState({ choose: null, presented: null, standalone: false });
      resolve(choice);
    };
  });
  store.setState({ choose, presented: inMain ? markPresented : null, standalone: !inMain });
  if (inMain) {
    // A failed renderer may have lost its modal host while the module and
    // pending settings survive. Missing presentation keeps the safe choice.
    await bounded(presented, 500).catch(() => choose(safe));
  } else {
    const { t, text } = documentTranslator();
    void invoke<number>("quit_question", {
      title: text(standalone.title),
      body: text(standalone.body),
      labels: standalone.choices.map(([, label]) => t(label)),
    }).then((index) => choose(standalone.choices[index]?.[0] ?? safe), () => choose(safe));
  }
  return decision;
}

const RUNNING_OPERATION: Record<MutationKind, MessageKey> = {
  "destination-copy": "quit.runningCopy",
  "destination-move": "quit.runningMove",
  delete: "quit.runningDelete",
  restore: "quit.runningRestore",
  "trash-empty": "quit.runningEmpty",
};

/** What quit would stop that the user started and may want finished, one line
 * each: a file operation, and managed-tool downloads and installs. Automatic
 * derived work (previews, snapshots, similarity, faces, transcripts) and
 * source checks and scans are left out: they resume by themselves at the next
 * launch and lose nothing, and asking about them would interrupt nearly every
 * quit. Work the user is already cancelling is not asked about either. */
export function runningWork(): Message[] {
  const work: Message[] = [];
  const operation = useMutationStore.getState();
  if (operation.progress !== null && !operation.cancelling) work.push(message(RUNNING_OPERATION[operation.progress.kind]));
  const { installing, entries } = useBinariesStore.getState();
  for (const [id, install] of Object.entries(installing)) {
    if (install.cancelling) continue;
    const entry = entries.find((candidate) => candidate.id === id);
    const tool = entry === undefined ? id : toolName(entry);
    const phase = install.progress?.phase;
    work.push(message(phase === "verify" || phase === "install" ? "quit.toolInstalling" : "quit.toolDownloading", { tool }));
  }
  return work;
}

function lines(messages: Message[]): Message {
  return messages.reduceRight((rest, first) => message("quit.lines", { first, rest }));
}

/** Whether the quit may stop the running work: true when none is running or
 * the user chose Stop and quit. Stopping itself is the native exit's, so work
 * that finished while the question was open just lets the quit go on. */
async function runningWorkMayStop(revision: number): Promise<boolean> {
  const work = runningWork();
  if (work.length === 0) return true;
  const body = lines(work);
  useQuitRunningWorkStore.setState({ body });
  return ask(useQuitRunningWorkStore, revision, false, {
    title: message("quit.runningWorkTitle"),
    body,
    choices: [[false, "quit.keepRunning"], [true, "quit.stopAndQuit"]],
  });
}

/** Whether the quit may go on past the Settings draft: true when there are no
 * unsaved edits or the user chose Discard. Checked after the required save, so
 * a Settings save that was in flight has settled. */
async function settingsMayBeDiscarded(revision: number): Promise<boolean> {
  if (!settingsDraftIsDirty(useSettingsStore.getState())) return true;
  // Like the save decision: a prompt that cannot be shown keeps editing.
  const discard = await ask(useQuitDiscardStore, revision, false, {
    title: message("settings.discardTitle"),
    body: message("settings.discardMessage"),
    choices: [[false, "settings.keepEditing"], [true, "settings.discard"]],
  });
  if (!discard || sessionEnding || revision !== sessionRevision) return false;
  useSettingsStore.getState().discardDraft();
  useAppShellStore.getState().closeUtility();
  return true;
}

// Missing presentation cancels, never discards.
function decide(revision: number): Promise<QuitChoice> {
  return ask(useQuitSaveStore, revision, "cancel", {
    title: message("quit.saveFailedTitle"),
    body: message("quit.saveFailedBody"),
    choices: [["cancel", "common.cancel"], ["retry", "common.retry"], ["quit", "quit.anyway"]],
  });
}

// The order: the required save comes first, as it always gets its attempt;
// then the running work; the Settings discard is last because Discard acts at
// once, so it follows every question that can still cancel the quit.
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
        const choice = await decide(revision);
        if (choice === "session" || sessionEnding || revision !== sessionRevision) return;
        if (choice === "cancel") { resumeStatePatchesAfterFailedShutdown(); return; }
        if (choice === "quit") break;
      }
    }
    if (sessionEnding || revision !== sessionRevision) return;
    if (!(await runningWorkMayStop(revision))) return;
    if (sessionEnding || revision !== sessionRevision) return;
    if (!(await settingsMayBeDiscarded(revision))) return;
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
  // The OS ending the session never asks; the draft is left as it is, and the
  // running work is not consent to an ordinary quit: the native session end
  // stops it on its own.
  useQuitDiscardStore.getState().choose?.(false);
  useQuitRunningWorkStore.getState().choose?.(false);
  try { await saveRequired(); } catch (error) { reportStatePatchFailure(error); }
  finally { await invoke("session_end_saved", { id }).catch(reportWindowCall("report session-end save")); }
}

/** The page lost its modal hosts: their questions cancel the quit. A
 * standalone question does not depend on them and stays. */
export function cancelQuitDecision(): void {
  if (!useQuitSaveStore.getState().standalone) useQuitSaveStore.getState().choose?.("cancel");
  if (!useQuitRunningWorkStore.getState().standalone) useQuitRunningWorkStore.getState().choose?.(false);
  if (!useQuitDiscardStore.getState().standalone) useQuitDiscardStore.getState().choose?.(false);
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
