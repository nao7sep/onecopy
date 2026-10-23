import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  anchorContextFromPayload,
  type AnchorContext,
} from "../models/mainSelection";
import type { SectionRecoveryContextPayload } from "../models/items";
import { visibleKeepMarks } from "../models/comparisonSession";
import { focusWhileActive, log, reportWindowCall, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import {
  broadcastComparison,
  comparisonProjectionIsCurrent,
  recoverComparisonDisplay,
  slotIndexForKey,
  useComparisonStore,
  visibleMembers,
  type ComparisonCommitResult,
  type ComparisonOpenResult,
  type ComparisonProjection,
} from "../state/comparison-store";
import { useIssuesStore } from "../state/issues-store";
import { useItemsStore } from "../state/items-store";
import { beginMainFeedback } from "../state/main-feedback-store";
import { message } from "../i18n/translate";
import { recordActionFailure } from "../state/notifications-store";
import { useMutationStore } from "../state/mutation-store";
import { restorePreviewAfterComparison } from "../state/preview-store";
import { useSectionsStore } from "../state/sections-store";
import { presentEscapedFailure, recordInterfaceFailure } from "../utils/failureSurface";
import { createEventInstaller } from "../utils/eventInstallation";
import { hasOpenModal, isTopmostModal } from "../utils/modalStack";
import {
  latestActivityOperationId,
  newActivityOperationId,
  recordActivity,
} from "../repositories/activity";
import { configNumber, confirmsTrashDelete } from "../models/config";
import { isHashKey } from "../models/items";
import { requestSeq } from "../state/request-seq";
import { isHelpShortcut, isSettingsShortcut } from "../utils/shortcuts";
import { isZoomIn, isZoomOut, isZoomReset } from "../utils/zoom";

const comparisonOpening = requestSeq();
let mainRecoveryAfterFamily: { sessionId: number; context: AnchorContext | null } | null = null;

function appConfig(): Record<string, unknown> {
  return useAppStore.getState().appData?.config ?? {};
}

function configConfirmsTrash(): boolean {
  return confirmsTrashDelete(useAppStore.getState().appData?.config);
}

function maximumImages(): number {
  const value = configNumber(useAppStore.getState().appData?.config, "maximumImagesInComparison");
  return Math.max(2, Math.floor(value ?? 2));
}

async function refreshLibrary(): Promise<void> {
  await Promise.all([
    useItemsStore.getState().refresh(),
    useSectionsStore.getState().loadCounts(),
  ]);
}

async function restoreMainFocus(current: () => boolean = () => true): Promise<void> {
  await focusWhileActive("main");
  if (!current()) return;
  document.getElementById("main-item-area")?.focus();
}

async function focusComparison(current: () => boolean): Promise<void> {
  await focusWhileActive("main");
  if (!current()) return;
  document.getElementById("comparison-item-area")?.focus();
}

/** Main keeps its own selection while Comparison runs and after it closes or
 * is cancelled; only a completed decision moves Main's anchor. */
async function applyResult(
  result: ComparisonCommitResult | null,
  sessionId: number,
): Promise<void> {
  if (result === null) return;
  const current = () => useComparisonStore.getState().sessionId === sessionId;
  // The durable result refreshes the library even if its session was replaced.
  await Promise.all([refreshLibrary(), useIssuesStore.getState().load()]);
  if (!current()) return;
  if (result.kind === "failed") {
    await reconcileComparisonMembership();
    return;
  }
  if (result.kind === "continued") {
    await reconcileComparisonMembership();
    return;
  }
  const recovery = mainRecoveryAfterFamily?.sessionId === sessionId
    ? mainRecoveryAfterFamily.context : null;
  mainRecoveryAfterFamily = null;
  await useItemsStore.getState().selectAfterFamily(recovery, current);
  if (!current()) return;
  await restorePreviewAfterComparison(current);
  if (!current()) return;
  await restoreMainFocus(current);
}

export async function openComparison(
  hash: string,
  entryAnchor: string | null = hash,
): Promise<ComparisonOpenResult> {
  const fresh = comparisonOpening.begin();
  let openedSessionId: number | null = null;
  const result = await useComparisonStore
    .getState()
    .openGroup(
      hash,
      entryAnchor,
      maximumImages(),
      appConfig(),
      (sessionId) => { openedSessionId = sessionId; },
    );
  if (openedSessionId === null && (!fresh() || result === "opened")) return result;
  const sessionId = openedSessionId ?? useComparisonStore.getState().sessionId;
  // A rejected later open attempt does not retire this admitted session.
  // Only an actual replacement (or closing, for preparation) does.
  const current = () => useComparisonStore.getState().sessionId === sessionId;
  if (result === "opened") {
    const ownsOpenSession = () => current() && useComparisonStore.getState().open;
    if (!ownsOpenSession()) return result;
    // Where Main lands after this session; never an earlier session's spot.
    mainRecoveryAfterFamily = { sessionId, context: null };
    await focusComparison(ownsOpenSession);
    if (!ownsOpenSession()) return result;
    const items = useItemsStore.getState();
    const section = items.selected;
    const hashes = useComparisonStore.getState().originalMemberHashes;
    if (section !== null) {
      try {
        const context = await invoke<SectionRecoveryContextPayload | null>(
          "get_section_family_context",
          {
            kind: section.kind,
            month: section.month,
            sort: items.currentSort(),
            memberHashes: hashes,
          },
        );
        if (!ownsOpenSession()) return result;
        mainRecoveryAfterFamily = { sessionId, context: anchorContextFromPayload(context) };
      } catch (error) {
        if (!ownsOpenSession()) return result;
        log.error("comparison return position failed", toErrorFields(error));
        await useComparisonStore.getState().close();
        if (!current()) return "failed";
        await restorePreviewAfterComparison(current);
        if (!current()) return "failed";
        await restoreMainFocus(current);
        if (!current()) return "failed";
        recordInterfaceFailure(message("comparison.prepareFailed"), error);
        return "failed";
      }
    }
  }
  if (result === "failed") {
    await restorePreviewAfterComparison(current);
    if (!current()) return result;
    await restoreMainFocus(current);
  }
  return result;
}

export async function requestComparisonFromMain(): Promise<void> {
  const feedback = beginMainFeedback("comparison");
  const { selectedKeys, selectedItem } = useItemsStore.getState();
  // Whether the selection is one similar group is the core's decision
  // (comparison_selection_valid); an unhashed anchor cannot open one at all.
  const hash = selectedItem !== null && isHashKey(selectedItem) ? selectedItem : null;
  if (hash === null) {
    feedback.finish({ tone: "normal", text: message("comparison.needsOneGroup") });
    return;
  }
  const operationId = newActivityOperationId("comparison");
  recordActivity({
    kind: "started",
    owner: "comparison",
    operationId,
    causeId: latestActivityOperationId("selection"),
    current: "running",
    reason: "user",
    lane: "image",
    itemCount: selectedKeys.size,
  });
  try {
    const valid = await invoke<boolean>("comparison_selection_valid", { hashes: [...selectedKeys] });
    if (!feedback.current()) {
      recordActivity({ kind: "stale", owner: "comparison", operationId,
        previous: "running", current: "stale", reason: "superseded" });
      return;
    }
    if (!valid) {
      feedback.finish({ tone: "normal", text: message("comparison.needsOneGroup") });
      recordActivity({
        kind: "completed",
        owner: "comparison",
        operationId,
        previous: "running",
        current: "idle",
        reason: "dependency",
      });
      return;
    }
  } catch (error) {
    log.error("comparison admission failed", toErrorFields(error));
    const failure = message("comparison.admissionFailed");
    feedback.finish({ tone: "danger", text: failure });
    recordActionFailure("comparison-admission-failed", failure, error);
    recordActivity({
      kind: "failed",
      owner: "comparison",
      operationId,
      previous: "running",
      current: "failed",
      reason: "error",
    });
    return;
  }
  const result = await openComparison(hash, selectedItem);
  if (result === "unavailable") {
    feedback.finish({ tone: "normal", text: message("comparison.nothingLeft") });
    recordActivity({
      kind: "completed",
      owner: "comparison",
      operationId,
      previous: "running",
      current: "idle",
      reason: "dependency",
    });
  } else if (result === "failed") {
    feedback.finish({ tone: "danger", text: message("comparison.openFailedSeeIssues") });
    recordActivity({
      kind: "failed",
      owner: "comparison",
      operationId,
      previous: "running",
      current: "failed",
      reason: "error",
    });
  } else {
    feedback.finish();
    recordActivity({
      kind: "opened",
      owner: "comparison",
      operationId,
      previous: "running",
      current: "open",
      reason: "completion",
    });
  }
}

export async function closeComparison(): Promise<void> {
  if (useComparisonStore.getState().busy) {
    await useMutationStore.getState().cancel();
    return;
  }
  comparisonOpening.begin();
  const sessionId = useComparisonStore.getState().sessionId;
  const current = () => useComparisonStore.getState().sessionId === sessionId;
  await useComparisonStore.getState().close();
  await refreshLibrary();
  if (!current()) return;
  await restorePreviewAfterComparison(current);
  if (!current()) return;
  await restoreMainFocus(current);
  if (!current()) return;
  recordActivity({
    kind: "closed",
    owner: "comparison",
    previous: "open",
    current: "closed",
    reason: "user",
  });
}

export async function decideComparisonPage(
  permanent: boolean,
  trashAll = false,
): Promise<void> {
  const state = useComparisonStore.getState();
  if (
    !trashAll &&
    visibleKeepMarks(state.selected, visibleMembers(state)).size === 0
  ) {
    await closeComparison();
    return;
  }
  await applyResult(
    await state.requestPageDecision(permanent, trashAll),
    state.sessionId,
  );
}

export async function deleteComparisonSelection(
  permanent: boolean,
): Promise<void> {
  const state = useComparisonStore.getState();
  await applyResult(
    await state.requestSelectionDelete(permanent, configConfirmsTrash()),
    state.sessionId,
  );
}

export async function confirmComparisonAction(): Promise<void> {
  const state = useComparisonStore.getState();
  await applyResult(await state.confirmPendingAction(), state.sessionId);
}

export async function retryComparisonFailure(): Promise<void> {
  const state = useComparisonStore.getState();
  await applyResult(
    await state.retryFailure(configConfirmsTrash()),
    state.sessionId,
  );
}

export async function reconcileComparisonMembership(): Promise<void> {
  const store = useComparisonStore.getState();
  if (!store.open || store.busy) return;
  const sessionId = store.sessionId;
  const ownsSession = () => useComparisonStore.getState().sessionId === sessionId;
  const requested = store.members.map((member) => member.hash);
  try {
    const live = await invoke<string[]>("comparison_live_hashes", {
      hashes: requested,
    });
    const current = useComparisonStore.getState();
    if (!ownsSession() || !current.open || current.busy) return;
    const stillOpen = await current.reconcileLiveMembers(live);
    if (!stillOpen && ownsSession()) {
      await refreshLibrary();
      if (!ownsSession()) return;
      await restorePreviewAfterComparison(ownsSession);
      if (!ownsSession()) return;
      await restoreMainFocus(ownsSession);
    }
  } catch (error) {
    if (!ownsSession()) return;
    log.warn("comparison membership refresh failed", toErrorFields(error));
    recordInterfaceFailure(message("comparison.membershipRefreshFailed"), error);
    useComparisonStore.setState({
      message: message("comparison.membershipRefreshFailed"),
    });
  }
}

export function comparisonKeyIsRoutable(event: {
  key: string;
  repeat?: boolean;
  shiftKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}, visibleCount = 36): boolean {
  const directIndex = slotIndexForKey(event);
  if (directIndex >= 0) return directIndex < visibleCount;
  const command = event.metaKey === true || event.ctrlKey === true;
  if (
    command &&
    event.altKey !== true &&
    event.shiftKey !== true &&
    event.key.toLowerCase() === "a"
  ) {
    return true;
  }
  if (command || event.altKey === true) return false;
  if (
    event.key === "Enter" ||
    event.key === "Delete" ||
    event.key === "Backspace" ||
    event.key === "Home" ||
    event.key === "End" ||
    event.key.startsWith("Arrow")
  ) {
    return true;
  }
  if (event.shiftKey === true) return false;
  return (
    event.key === "Escape" ||
    event.key === "PageDown" ||
    event.key === "PageUp" ||
    event.key === " "
  );
}

/** The app shortcuts that work in Comparison besides its own keys: the
 * shortcuts list and Settings, which open in Main, and zoom, which scales
 * Main. Every Comparison window hands them to Main (`comparison://app-key`),
 * so they do the same thing whichever window has focus (window-conventions). */
export function comparisonAppKey(event: KeyboardEvent): "dialog" | "zoom" | null {
  if (isHelpShortcut(event) || isSettingsShortcut(event)) return "dialog";
  if (isZoomIn(event) || isZoomOut(event) || isZoomReset(event)) return "zoom";
  return null;
}

/** The forwarded key as Main's own shortcut handlers receive it. */
export type ComparisonAppKey = ComparisonProjection & {
  key: string;
  code: string;
  shiftKey: boolean;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
};

// The local view and commands forwarded by secondary displays belong to the
// same blocking surface; a confirmation above it still suppresses both.
export const comparisonLayerToken = {};

export function handleComparisonKey(event: {
  key: string;
  repeat?: boolean;
  shiftKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}): boolean {
  const store = useComparisonStore.getState();
  if (!store.open || (hasOpenModal() && !isTopmostModal(comparisonLayerToken))) return false;
  if (store.busy) {
    if (
      event.key === "Escape" &&
      event.metaKey !== true &&
      event.ctrlKey !== true &&
      event.altKey !== true &&
      event.shiftKey !== true
    ) {
      void useMutationStore.getState().cancel();
      return true;
    }
    return false;
  }
  if (!comparisonKeyIsRoutable(event, visibleMembers(store).length)) {
    return false;
  }
  if (
    event.repeat === true &&
    (event.key === "Enter" ||
      event.key === "Delete" ||
      event.key === "Backspace" ||
      event.key === " ")
  ) {
    return true;
  }
  const slotIndex = slotIndexForKey(event);
  if (slotIndex >= 0) {
    store.selectSlot(slotIndex, "toggle");
    return true;
  }
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "a") {
    store.markAll();
    return true;
  }
  if (event.key === "Enter") {
    void decideComparisonPage(event.shiftKey === true);
    return true;
  }
  if (event.key === "Delete" || event.key === "Backspace") {
    void deleteComparisonSelection(event.shiftKey === true);
    return true;
  }
  if (event.key === "Escape") {
    void closeComparison();
    return true;
  }
  if (event.key === "PageDown") {
    store.nextPage();
    return true;
  }
  if (event.key === "PageUp") {
    store.prevPage();
    return true;
  }
  if (event.key === "Home" || event.key === "End") {
    store.activateBound(
      event.key === "Home" ? "first" : "last",
      event.shiftKey === true,
    );
    return true;
  }
  if (event.key.startsWith("Arrow")) {
    const direction = event.key.slice(5).toLowerCase();
    if (
      direction === "left" ||
      direction === "right" ||
      direction === "up" ||
      direction === "down"
    ) {
      store.moveActive(direction, event.shiftKey === true);
      return true;
    }
  }
  // Space is taken and does nothing: nothing enlarges an image from
  // Comparison. Seeing one whole image as large as possible is left out of
  // the workflow for speed; the composition is already on the display, and
  // click-and-hold inspection gives the close look.
  if (event.key === " ") return true;
  return false;
}

const installEvents = createEventInstaller(
  async (listeners) => {
    if (getCurrentWindow().label !== "main") return;
    await listeners.listen<ComparisonProjection & { slice: number; aspect: number }>("comparison://layout", (event) => {
      const store = useComparisonStore.getState();
      // Geometry belongs to the physical display in this session, and does
      // not become stale merely because its keep marks changed meanwhile.
      if (!store.open || event.payload.sessionId !== store.sessionId) return;
      store.setDisplayAspect(event.payload.slice, event.payload.aspect);
    });
    await listeners.listen<ComparisonProjection & {
      key: string;
      repeat?: boolean;
      shiftKey?: boolean;
      metaKey?: boolean;
      ctrlKey?: boolean;
      altKey?: boolean;
    }>("comparison://key", (event) => {
      if (!comparisonProjectionIsCurrent(event.payload)) return;
      handleComparisonKey(event.payload);
    });
    // Replayed as the key itself, so Main's own shortcut handlers act on it
    // exactly as if it had been pressed in Main; a dialog needs Main's focus.
    await listeners.listen<ComparisonAppKey>("comparison://app-key", async (event) => {
      // A newer keep mark does not outdate a zoom or a dialog; another session does.
      const store = useComparisonStore.getState();
      if (!store.open || event.payload.sessionId !== store.sessionId) return;
      const { key, code, shiftKey, metaKey, ctrlKey, altKey } = event.payload;
      const replay = new KeyboardEvent("keydown", { key, code, shiftKey, metaKey, ctrlKey, altKey, bubbles: true, cancelable: true });
      if (comparisonAppKey(replay) === "dialog") {
        await focusWhileActive("main").catch(reportWindowCall("main focus"));
      }
      window.dispatchEvent(replay);
    });
    await listeners.listen<ComparisonProjection & {
      slotIndex: number;
      hash: string;
      mode: "activate" | "toggle" | "range";
    }>("comparison://select", (event) => {
      const store = useComparisonStore.getState();
      if (!comparisonProjectionIsCurrent(event.payload) || store.busy || (hasOpenModal() && !isTopmostModal(comparisonLayerToken))) return;
      if (visibleMembers(store)[event.payload.slotIndex]?.hash !== event.payload.hash) return;
      store.selectSlot(event.payload.slotIndex, event.payload.mode);
    });
    await listeners.listen("comparison://ready", () => {
      broadcastComparison();
    });
    // Alt+F4 or Close Window on one of Comparison's displays leaves
    // Comparison as Escape does; Rust prevents the close itself
    // (fullscreen::session_close_event), and Comparison owns its windows.
    await listeners.listen("comparison://close-requested", () => {
      if (useComparisonStore.getState().open) void closeComparison();
    });
    await listeners.listen<{ slice: number }>("comparison://display-failed", (event) => {
      void recoverComparisonDisplay(event.payload.slice);
    });
  },
  (error) => {
    log.warn("comparison display wiring failed", toErrorFields(error));
    recordInterfaceFailure(message("comparison.displayControlsUnavailable"), error);
    presentEscapedFailure(message("comparison.displayControlsUnavailableReload"));
  },
);

export function installComparisonEventWiring(): Promise<void> {
  return installEvents();
}
