import { emit } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import {
  availableMonitors,
  currentMonitor,
  getCurrentWindow,
  PhysicalPosition,
  PhysicalSize,
} from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { create } from "zustand";
import {
  COMPARISON_DIRECT_KEYS,
  activateSelection,
  activePage,
  activeSelection,
  chunkMembers,
  comparisonPages,
  directKeyIndex,
  displayCapacities,
  spatialTarget,
  updateSelection,
  type ComparisonMember,
  type ComparisonSelection,
  type ComparisonSelectionMode,
} from "../models/comparisonSession";
import { log, reportWindowCall, toErrorFields } from "../repositories";
import { monitorKey, orderMonitors, priorityFromState } from "../utils/screens";
import { requestSeq } from "./request-seq";
import { recordInterfaceFailure } from "../utils/failureSurface";

export type GroupMember = ComparisonMember;
export type ComparisonOpenResult = "opened" | "unavailable" | "failed";

export function slotIndexForKey(
  event: Parameters<typeof directKeyIndex>[0],
): number {
  return directKeyIndex(event);
}

export interface ComparisonSlotState {
  member: GroupMember;
  slotKey: string | null;
  selected: boolean;
  anchor: boolean;
}

export interface ComparisonBroadcast {
  chunks: ComparisonSlotState[][];
  page: number;
  pageCount: number;
  remainingCount: number;
  portraitDominant: boolean;
}

export type ComparisonActionKind = "page" | "selection";

interface ComparisonAction {
  kind: ComparisonActionKind;
  permanent: boolean;
  keepHashes: string[];
  targetHashes: string[];
}

interface ComparisonFailure extends ComparisonAction {
  message: string;
}

interface ComparisonState extends ComparisonSelection {
  sessionId: number;
  open: boolean;
  members: GroupMember[];
  originalMemberHashes: string[];
  page: number;
  maximumImages: number;
  displayCount: number;
  displayAspects: number[];
  capacities: number[];
  portraitDominant: boolean;
  spreadCount: number;
  busy: boolean;
  message: string | null;
  pendingAction: ComparisonAction | null;
  failure: ComparisonFailure | null;
  openGroup: (
    hash: string,
    initialSelection?: Iterable<string>,
    entryAnchor?: string | null,
    maximumImages?: number,
    screenState?: Record<string, unknown>,
  ) => Promise<ComparisonOpenResult>;
  selectSlot: (slotIndex: number, mode: ComparisonSelectionMode) => void;
  moveSelection: (
    direction: "left" | "right" | "up" | "down",
    extend: boolean,
  ) => void;
  selectBound: (bound: "first" | "last", extend: boolean) => void;
  selectAll: () => void;
  nextPage: () => void;
  prevPage: () => void;
  unlinkSelected: () => Promise<"open" | "closed" | null>;
  requestPageDecision: (
    permanent: boolean,
    configConfirms?: boolean,
    trashAll?: boolean,
  ) => Promise<ComparisonCommitResult | null>;
  requestSelectionDelete: (
    permanent: boolean,
    configConfirms?: boolean,
  ) => Promise<ComparisonCommitResult | null>;
  confirmPendingAction: () => Promise<ComparisonCommitResult | null>;
  cancelPendingAction: () => void;
  retryFailure: (
    configConfirms?: boolean,
  ) => Promise<ComparisonCommitResult | null>;
  reconcileLiveMembers: (liveHashes: Iterable<string>) => Promise<boolean>;
  close: () => Promise<void>;
}

export type ComparisonCommitResult =
  | { kind: "failed" }
  | { kind: "continued" }
  | { kind: "completed" };

type MonitorList = Awaited<ReturnType<typeof availableMonitors>>;

let sessionOtherMonitors: MonitorList = [];

function selectionFrom(state: ComparisonState): ComparisonSelection {
  return {
    selected: state.selected,
    anchors: state.anchors,
    anchor: state.anchor,
    rangeOrigin: state.rangeOrigin,
    rangeBase: state.rangeBase,
  };
}

export function visibleMembers(
  state: Pick<
    ComparisonState,
    "members" | "page" | "maximumImages" | "displayCount"
  >,
): GroupMember[] {
  return activePage(
    state.members,
    state.page,
    state.maximumImages,
    state.displayCount,
  ).members;
}

function viewPatch(
  state: ComparisonState,
  members = state.members,
  requestedPage = state.page,
  preferredSelected: Iterable<string> = [],
  preferredAnchor: string | null = null,
): Partial<ComparisonState> {
  const live = new Set(members.map((member) => member.hash));
  const liveSelection: ComparisonSelection = {
    selected: new Set([...state.selected].filter((hash) => live.has(hash))),
    anchors: new Set([...state.anchors].filter((hash) => live.has(hash))),
    anchor:
      state.anchor !== null && live.has(state.anchor) ? state.anchor : null,
    rangeOrigin:
      state.rangeOrigin !== null && live.has(state.rangeOrigin)
        ? state.rangeOrigin
        : null,
    rangeBase: new Set([...state.rangeBase].filter((hash) => live.has(hash))),
  };
  const pages = comparisonPages(
    members,
    state.maximumImages,
    state.displayCount,
  );
  const page = Math.min(
    Math.max(0, requestedPage),
    Math.max(0, pages.length - 1),
  );
  const active = pages[page] ?? {
    members: [],
    portraitDominant: false,
    perDisplay: 4,
  };
  let recoveredAnchor = preferredAnchor;
  if (
    recoveredAnchor === null &&
    state.anchor !== null &&
    !live.has(state.anchor)
  ) {
    const oldIndex = state.members.findIndex(
      (member) => member.hash === state.anchor,
    );
    const visible = new Set(active.members.map((member) => member.hash));
    const next = state.members.slice(oldIndex + 1).map((member) => member.hash);
    const previous = state.members
      .slice(0, Math.max(0, oldIndex))
      .reverse()
      .map((member) => member.hash);
    recoveredAnchor =
      next.find(
        (hash) => visible.has(hash) && liveSelection.selected.has(hash),
      ) ??
      previous.find(
        (hash) => visible.has(hash) && liveSelection.selected.has(hash),
      ) ??
      next.find((hash) => visible.has(hash)) ??
      previous.find((hash) => visible.has(hash)) ??
      null;
  }
  const selection = activateSelection(
    liveSelection,
    active.members,
    preferredSelected,
    recoveredAnchor,
  );
  const capacities = displayCapacities(
    active.members.length,
    active.perDisplay,
    state.displayCount,
  );
  return {
    members,
    page,
    ...selection,
    capacities,
    portraitDominant: active.portraitDominant,
    spreadCount: Math.max(0, capacities.length - 1),
  };
}

function slotsFor(state: ComparisonState): ComparisonSlotState[] {
  return visibleMembers(state).map((member, index) => ({
    member,
    slotKey: COMPARISON_DIRECT_KEYS[index] ?? null,
    selected: state.selected.has(member.hash),
    anchor: state.anchor === member.hash,
  }));
}

export function comparisonChunks(
  state: ComparisonState,
): ComparisonSlotState[][] {
  return chunkMembers(slotsFor(state), state.capacities);
}

export function broadcastComparison(): void {
  const state = useComparisonStore.getState();
  if (!state.open) return;
  const pages = comparisonPages(
    state.members,
    state.maximumImages,
    state.displayCount,
  );
  const payload: ComparisonBroadcast = {
    chunks: comparisonChunks(state),
    page: state.page,
    pageCount: pages.length,
    remainingCount: state.members.length,
    portraitDominant: state.portraitDominant,
  };
  void emit("comparison://state", payload);
}

function pageForAnchor(
  members: GroupMember[],
  hash: string,
  maximumImages: number,
  displayCount: number,
): number {
  const pages = comparisonPages(members, maximumImages, displayCount);
  const found = pages.findIndex((page) =>
    page.members.some((member) => member.hash === hash),
  );
  return found < 0 ? 0 : found;
}

async function resolveMonitors(
  screenState: Record<string, unknown>,
): Promise<{ hostAspect: number; others: MonitorList }> {
  try {
    const monitors = orderMonitors(
      await availableMonitors(),
      priorityFromState(screenState),
    );
    const hosting = await currentMonitor();
    const hostKey = hosting === null ? null : monitorKey(hosting);
    const host =
      hosting ??
      (hostKey === null ? monitors[0] : monitors.find((monitor) => monitorKey(monitor) === hostKey)) ??
      null;
    return {
      hostAspect:
        host === null || host.size.height <= 0
          ? 16 / 9
          : host.size.width / host.size.height,
      others:
        hostKey === null
          ? monitors.slice(1)
          : monitors.filter((monitor) => monitorKey(monitor) !== hostKey),
    };
  } catch (error) {
    log.warn(
      "monitor query failed; staying on the main display",
      toErrorFields(error),
    );
    recordInterfaceFailure("Couldn’t read the connected displays for Comparison.");
    return { hostAspect: 16 / 9, others: [] };
  }
}

const fullscreenTransitions = new Map<string, Promise<void>>();

function setComparisonFullscreen(
  label: string,
  enable: boolean,
): Promise<void> {
  const previous = fullscreenTransitions.get(label) ?? Promise.resolve();
  const next = previous
    .catch(() => undefined)
    .then(() =>
      invoke<void>("set_window_simple_fullscreen", { label, enable }),
    );
  fullscreenTransitions.set(label, next);
  const clear = () => {
    if (fullscreenTransitions.get(label) === next) {
      fullscreenTransitions.delete(label);
    }
  };
  void next.then(clear, clear);
  return next;
}

async function showSpread(monitors: MonitorList): Promise<void> {
  for (let index = 0; index < monitors.length; index += 1) {
    const label = `comparison-${index + 1}`;
    const monitor = monitors[index];
    try {
      const existing = await WebviewWindow.getByLabel(label);
      if (existing !== null) {
        await existing.setPosition(
          new PhysicalPosition(monitor.position.x, monitor.position.y),
        );
        await existing.setSize(
          new PhysicalSize(monitor.size.width, monitor.size.height),
        );
        await existing.show();
        await setComparisonFullscreen(label, true);
        continue;
      }
      const scale = monitor.scaleFactor || 1;
      const created = new WebviewWindow(label, {
        url: `index.html?view=comparison&slice=${index + 1}`,
        title: "OneCopy Comparison",
        x: monitor.position.x / scale,
        y: monitor.position.y / scale,
        width: monitor.size.width / scale,
        height: monitor.size.height / scale,
        decorations: false,
        alwaysOnTop: true,
        skipTaskbar: true,
        resizable: false,
        focus: false,
        visible: false,
      });
      void created.once("tauri://created", () => {
        void queueComparisonLifecycle(async () => {
          const state = useComparisonStore.getState();
          if (!state.open || index >= state.spreadCount) {
            await created
              .close()
              .catch(reportWindowCall("unused comparison close"));
            return;
          }
          try {
            await created.show();
            await setComparisonFullscreen(label, true);
          } catch (error) {
            log.warn("comparison display became unavailable", {
              label,
              ...toErrorFields(error),
            });
            recordInterfaceFailure(
              "A Comparison display became unavailable. The session was rearranged.",
            );
            void recoverDisplays(index + 1);
            return;
          }
          await getCurrentWindow()
            .setFocus()
            .catch(reportWindowCall("main setFocus"));
        });
      });
      void created.once("tauri://error", (event) => {
        log.warn("comparison window creation failed", {
          label,
          error: { message: String(event.payload) },
        });
        recordInterfaceFailure(
          "A Comparison display could not open. The session was rearranged.",
        );
        recoverDisplays(index + 1);
      });
    } catch (error) {
      log.warn("comparison display became unavailable", {
        label,
        ...toErrorFields(error),
      });
      recordInterfaceFailure(
        "A Comparison display became unavailable. The session was rearranged.",
      );
      recoverDisplays(index + 1);
      return;
    }
  }
}

async function hideSpread(first: number, last: number): Promise<void> {
  for (let index = first; index <= last; index += 1) {
    const label = `comparison-${index}`;
    const window = await WebviewWindow.getByLabel(label).catch((error) => {
      reportWindowCall("comparison window lookup")(error);
      return null;
    });
    if (window === null) continue;
    await setComparisonFullscreen(label, false).catch(
      reportWindowCall("comparison leave fullscreen"),
    );
    await window.hide().catch(reportWindowCall("comparison hide"));
  }
}

async function closeSpread(first: number, last: number): Promise<void> {
  for (let index = first; index <= last; index += 1) {
    const label = `comparison-${index}`;
    const window = await WebviewWindow.getByLabel(label).catch((error) => {
      reportWindowCall("comparison window lookup")(error);
      return null;
    });
    if (window === null) continue;
    await setComparisonFullscreen(label, false).catch(
      reportWindowCall("comparison leave fullscreen"),
    );
    await window.close().catch(reportWindowCall("comparison close"));
  }
}

let comparisonLifecycle: Promise<void> = Promise.resolve();

function queueComparisonLifecycle(action: () => Promise<void>): Promise<void> {
  const next = comparisonLifecycle.then(action, action);
  comparisonLifecycle = next.catch(() => undefined);
  return next;
}

async function hidePreviewWindowForComparison(): Promise<void> {
  const preview = await WebviewWindow.getByLabel("preview").catch((error) => {
    reportWindowCall("preview lookup")(error);
    return null;
  });
  if (preview !== null) {
    await preview.hide().catch(reportWindowCall("preview hide"));
  }
}

function synchronizeSpread(previousSpreadCount: number): void {
  const state = useComparisonStore.getState();
  if (!state.open) return;
  const needed = state.spreadCount;
  broadcastComparison();
  void queueComparisonLifecycle(async () => {
    await showSpread(sessionOtherMonitors.slice(0, needed));
    if (previousSpreadCount > needed) {
      await hideSpread(needed + 1, previousSpreadCount);
    }
    await getCurrentWindow()
      .setFocus()
      .catch(reportWindowCall("main setFocus"));
  });
}

function recoverDisplays(failedSpreadIndex: number): Promise<void> {
  const state = useComparisonStore.getState();
  if (!state.open) return Promise.resolve();
  const previousSpreadCount = state.spreadCount;
  sessionOtherMonitors = sessionOtherMonitors.filter(
    (_, index) => index !== failedSpreadIndex - 1,
  );
  const displayAspects = state.displayAspects.filter(
    (_, index) => index === 0 || index !== failedSpreadIndex,
  );
  const provisional = {
    ...state,
    displayCount: sessionOtherMonitors.length + 1,
    displayAspects,
  };
  useComparisonStore.setState({
    displayCount: provisional.displayCount,
    displayAspects: provisional.displayAspects,
    ...viewPatch(provisional),
    message:
      "A comparison display became unavailable. The remaining images were rearranged.",
  });
  broadcastComparison();
  const needed = useComparisonStore.getState().spreadCount;
  return queueComparisonLifecycle(async () => {
    await closeSpread(1, previousSpreadCount);
    await showSpread(sessionOtherMonitors.slice(0, needed));
    await getCurrentWindow()
      .setFocus()
      .catch(reportWindowCall("main setFocus"));
  });
}

export function recoverComparisonDisplay(
  failedSpreadIndex: number,
): Promise<void> {
  return recoverDisplays(failedSpreadIndex);
}

export function closeComparisonAfterMainRendererFailure(): void {
  const state = useComparisonStore.getState();
  if (!state.open) return;
  const spreadCount = state.spreadCount;
  useComparisonStore.setState(closedComparisonState());
  void queueComparisonLifecycle(() => teardownComparison(spreadCount));
}

async function teardownComparison(spreadCount: number): Promise<void> {
  await hideSpread(1, spreadCount);
  await getCurrentWindow().setFocus().catch(reportWindowCall("main setFocus"));
}

const groupLoad = requestSeq();

export const useComparisonStore = create<ComparisonState>((set, get) => ({
  sessionId: 0,
  open: false,
  members: [],
  originalMemberHashes: [],
  page: 0,
  maximumImages: 16,
  displayCount: 1,
  displayAspects: [16 / 9],
  capacities: [4],
  portraitDominant: false,
  spreadCount: 0,
  selected: new Set(),
  anchors: new Set(),
  anchor: null,
  rangeOrigin: null,
  rangeBase: new Set(),
  busy: false,
  message: null,
  pendingAction: null,
  failure: null,

  openGroup: async (
    hash,
    initialSelection = [hash],
    entryAnchor = hash,
    maximumImages = 16,
    screenState = {},
  ) => {
    const fresh = groupLoad.begin();
    try {
      const [members, displays] = await Promise.all([
        invoke<GroupMember[]>("get_similar_group", { hash }),
        resolveMonitors(screenState),
      ]);
      if (!fresh()) return "opened";
      if (members.length < 2) {
        log.warn("similar group has fewer than two live members", { hash });
        return "unavailable";
      }
      sessionOtherMonitors = displays.others;
      const displayCount = displays.others.length + 1;
      const displayAspects = [
        displays.hostAspect,
        ...displays.others.map((monitor) =>
          monitor.size.height <= 0
            ? 16 / 9
            : monitor.size.width / monitor.size.height,
        ),
      ];
      const boundedMaximum = Math.max(2, Math.floor(maximumImages));
      const initialPage = pageForAnchor(
        members,
        entryAnchor ?? hash,
        boundedMaximum,
        displayCount,
      );
      const base = {
        ...get(),
        members,
        page: initialPage,
        maximumImages: boundedMaximum,
        displayCount,
        displayAspects,
        selected: new Set<string>(),
        anchors: new Set<string>(),
        anchor: null,
        rangeOrigin: null,
        rangeBase: new Set<string>(),
      };
      const next = viewPatch(
        base,
        members,
        initialPage,
        initialSelection,
        entryAnchor,
      );
      await queueComparisonLifecycle(async () => {
        if (!fresh()) return;
        set({
          sessionId: get().sessionId + 1,
          open: true,
          maximumImages: boundedMaximum,
          displayCount,
          displayAspects,
          originalMemberHashes: members.map((member) => member.hash),
          busy: false,
          message: null,
          pendingAction: null,
          failure: null,
          ...next,
        });
        broadcastComparison();
        await hidePreviewWindowForComparison();
        await showSpread(sessionOtherMonitors.slice(0, get().spreadCount));
        await getCurrentWindow()
          .setFocus()
          .catch(reportWindowCall("main setFocus"));
      });
      return "opened";
    } catch (error) {
      log.error("similar group load failed", toErrorFields(error));
      recordInterfaceFailure("Couldn’t open the similar-image group.");
      return "failed";
    }
  },

  selectSlot: (slotIndex, mode) => {
    const state = get();
    if (state.busy || state.pendingAction !== null) return;
    const visible = visibleMembers(state);
    const member = visible[slotIndex];
    if (member === undefined) return;
    set({
      ...updateSelection(selectionFrom(state), visible, member.hash, mode),
      message: null,
    });
    broadcastComparison();
  },

  moveSelection: (direction, extend) => {
    const state = get();
    if (state.busy || state.pendingAction !== null) return;
    const visible = visibleMembers(state);
    if (visible.length === 0) return;
    const current = Math.max(
      0,
      visible.findIndex((member) => member.hash === state.anchor),
    );
    const target = spatialTarget(
      current,
      direction,
      comparisonChunks(state).map((chunk) => chunk.length),
      state.portraitDominant,
      state.displayAspects,
    );
    if (target === current) return;
    set({
      ...updateSelection(
        selectionFrom(state),
        visible,
        visible[target].hash,
        extend ? "range" : "exclusive",
      ),
      message: null,
    });
    broadcastComparison();
  },

  selectBound: (bound, extend) => {
    const state = get();
    if (state.busy || state.pendingAction !== null) return;
    const visible = visibleMembers(state);
    const member = bound === "first" ? visible[0] : visible[visible.length - 1];
    if (member === undefined) return;
    set({
      ...updateSelection(
        selectionFrom(state),
        visible,
        member.hash,
        extend ? "range" : "exclusive",
      ),
      message: null,
    });
    broadcastComparison();
  },

  selectAll: () => {
    const state = get();
    if (state.busy || state.pendingAction !== null) return;
    const visible = visibleMembers(state);
    const selected = new Set(state.selected);
    for (const member of visible) selected.add(member.hash);
    const anchor = state.anchor ?? visible[0]?.hash ?? null;
    const anchors = new Set(state.anchors);
    for (const member of visible) anchors.delete(member.hash);
    if (anchor !== null) anchors.add(anchor);
    set({
      selected,
      anchors,
      anchor,
      rangeOrigin: anchor,
      rangeBase: activeSelection(selected, visible),
      message: null,
    });
    broadcastComparison();
  },

  nextPage: () => movePage(set, get, 1),
  prevPage: () => movePage(set, get, -1),

  unlinkSelected: async () => {
    const state = get();
    if (state.busy || state.pendingAction !== null) return null;
    const selected = activeSelection(state.selected, visibleMembers(state));
    if (selected.size === 0) {
      set({ message: "Select at least one image to mark as not similar." });
      return null;
    }
    set({ busy: true, message: null });
    const removed = new Set<string>();
    const failed: string[] = [];
    for (const hash of selected) {
      try {
        await invoke("similar_unlink", { hash });
        removed.add(hash);
      } catch (error) {
        log.error("similar unlink failed", { hash, ...toErrorFields(error) });
        const fileName = state.members.find((member) => member.hash === hash)?.fileName;
        recordInterfaceFailure(
          `Couldn’t mark ${fileName ?? "one image"} as not similar.`,
        );
        failed.push(hash);
      }
    }
    const current = get();
    const members = current.members.filter(
      (member) => !removed.has(member.hash),
    );
    if (members.length < 2) {
      const spreadCount = current.spreadCount;
      set(closedComparisonState());
      await queueComparisonLifecycle(() => teardownComparison(spreadCount));
      return "closed";
    }
    set({
      busy: false,
      ...viewPatch(current, members),
      message:
        failed.length === 0
          ? null
          : `${failed.length} image${failed.length === 1 ? "" : "s"} could not be marked as not similar.`,
    });
    synchronizeSpread(current.spreadCount);
    return "open";
  },

  requestPageDecision: async (
    permanent,
    configConfirms = false,
    trashAll = false,
  ) => {
    const state = get();
    if (state.busy || state.pendingAction !== null) return null;
    const visible = visibleMembers(state);
    const selected = trashAll
      ? new Set<string>()
      : activeSelection(state.selected, visible);
    if (!trashAll && selected.size === 0) {
      set({ message: "Select at least one image to keep." });
      return null;
    }
    const action: ComparisonAction = {
      kind: "page",
      permanent,
      keepHashes: visible
        .filter((member) => selected.has(member.hash))
        .map((member) => member.hash),
      targetHashes: visible
        .filter((member) => !selected.has(member.hash))
        .map((member) => member.hash),
    };
    return await requestAction(set, get, action, configConfirms);
  },

  requestSelectionDelete: async (permanent, configConfirms = false) => {
    const state = get();
    if (state.busy || state.pendingAction !== null) return null;
    const selected = activeSelection(state.selected, visibleMembers(state));
    if (selected.size === 0) return null;
    const action: ComparisonAction = {
      kind: "selection",
      permanent,
      keepHashes: [],
      targetHashes: [...selected],
    };
    return await requestAction(set, get, action, configConfirms);
  },

  confirmPendingAction: async () => {
    const action = get().pendingAction;
    if (action === null) return null;
    set({ pendingAction: null });
    return await executeAction(set, get, action);
  },

  cancelPendingAction: () => set({ pendingAction: null }),

  retryFailure: async (configConfirms = false) => {
    const failure = get().failure;
    if (failure === null || get().busy) return null;
    const action: ComparisonAction = {
      kind: failure.kind,
      permanent: failure.permanent,
      keepHashes: [],
      targetHashes: failure.targetHashes,
    };
    set({ failure: null });
    return await requestAction(set, get, action, configConfirms);
  },

  reconcileLiveMembers: async (liveHashes) => {
    const state = get();
    if (!state.open || state.busy) return false;
    const live = new Set(liveHashes);
    const members = state.members.filter((member) => live.has(member.hash));
    if (members.length === state.members.length) return true;
    if (members.length < 2) {
      const spreadCount = state.spreadCount;
      set(closedComparisonState());
      await queueComparisonLifecycle(() => teardownComparison(spreadCount));
      return false;
    }
    set({ ...viewPatch(state, members), message: null });
    synchronizeSpread(state.spreadCount);
    return true;
  },

  close: async () => {
    const state = get();
    if (!state.open || state.busy) return;
    const spreadCount = state.spreadCount;
    set(closedComparisonState());
    await queueComparisonLifecycle(() => teardownComparison(spreadCount));
  },
}));

function movePage(
  set: (partial: Partial<ComparisonState>) => void,
  get: () => ComparisonState,
  delta: number,
): void {
  const state = get();
  if (state.busy || state.pendingAction !== null) return;
  const pages = comparisonPages(
    state.members,
    state.maximumImages,
    state.displayCount,
  );
  const page = Math.min(pages.length - 1, Math.max(0, state.page + delta));
  if (page === state.page) return;
  const previousSpreadCount = state.spreadCount;
  set({ ...viewPatch(state, state.members, page), message: null });
  synchronizeSpread(previousSpreadCount);
}

async function requestAction(
  set: (partial: Partial<ComparisonState>) => void,
  get: () => ComparisonState,
  action: ComparisonAction,
  configConfirms: boolean,
): Promise<ComparisonCommitResult | null> {
  if (action.permanent || (configConfirms && action.targetHashes.length > 0)) {
    set({ pendingAction: action, message: null });
    return null;
  }
  return await executeAction(set, get, action);
}

async function executeAction(
  set: (partial: Partial<ComparisonState>) => void,
  get: () => ComparisonState,
  action: ComparisonAction,
): Promise<ComparisonCommitResult> {
  const before = get();
  const live = new Set(before.members.map((member) => member.hash));
  const keep = new Set(action.keepHashes.filter((hash) => live.has(hash)));
  const targets = action.targetHashes.filter((hash) => live.has(hash));
  let members = before.members.filter((member) => !keep.has(member.hash));
  set({
    busy: targets.length > 0,
    failure: null,
    message: null,
    ...viewPatch(before, members),
  });

  if (targets.length === 0) {
    return await finishAction(set, get, before, members);
  }

  let outcome: {
    cancelled: boolean;
    error: string | null;
    failedFiles: number;
    items: Array<{
      item: { hash: string | null; pathId: number | null };
      failedFiles: number;
    }>;
  };
  try {
    outcome = await invoke<typeof outcome>("delete_items", {
      items: targets.map((hash) => ({ hash, pathId: null })),
      permanent: action.permanent,
    });
  } catch (error) {
    log.error("comparison delete failed", toErrorFields(error));
    recordInterfaceFailure("The Comparison delete operation could not start.");
    const failure: ComparisonFailure = {
      ...action,
      keepHashes: [],
      targetHashes: targets,
      message:
        "The delete operation could not start. Retry targets only these images.",
    };
    set({
      busy: false,
      failure,
      ...viewPatch(get(), members, get().page, targets, targets[0] ?? null),
    });
    return { kind: "failed" };
  }

  const completed = new Set(
    outcome.items
      .filter((item) => item.failedFiles === 0 && item.item.hash !== null)
      .map((item) => item.item.hash as string),
  );
  members = members.filter((member) => !completed.has(member.hash));
  const remaining = targets.filter((hash) => !completed.has(hash));
  if (remaining.length > 0 || outcome.error !== null) {
    const detail =
      outcome.failedFiles > 0
        ? `${outcome.failedFiles} file${outcome.failedFiles === 1 ? "" : "s"} could not be deleted.`
        : outcome.cancelled
          ? "Deletion stopped safely."
          : (outcome.error ??
            `${remaining.length} image${remaining.length === 1 ? "" : "s"} remain.`);
    const failure: ComparisonFailure = {
      ...action,
      keepHashes: [],
      targetHashes: remaining,
      message: `${detail} Retry targets only the remaining images.`,
    };
    set({
      busy: false,
      failure,
      ...viewPatch(get(), members, get().page, remaining, remaining[0] ?? null),
    });
    synchronizeSpread(before.spreadCount);
    return { kind: "failed" };
  }

  return await finishAction(set, get, before, members);
}

async function finishAction(
  set: (partial: Partial<ComparisonState>) => void,
  get: () => ComparisonState,
  before: ComparisonState,
  members: GroupMember[],
): Promise<ComparisonCommitResult> {
  if (members.length < 2) {
    const spreadCount = get().spreadCount;
    set(closedComparisonState());
    await queueComparisonLifecycle(() => teardownComparison(spreadCount));
    return { kind: "completed" };
  }
  const current = get();
  set({
    busy: false,
    failure: null,
    pendingAction: null,
    ...viewPatch(current, members, current.page),
  });
  synchronizeSpread(before.spreadCount);
  return { kind: "continued" };
}

function closedComparisonState(): Partial<ComparisonState> {
  sessionOtherMonitors = [];
  return {
    open: false,
    members: [],
    originalMemberHashes: [],
    page: 0,
    maximumImages: 16,
    displayCount: 1,
    displayAspects: [16 / 9],
    capacities: [4],
    portraitDominant: false,
    spreadCount: 0,
    selected: new Set(),
    anchors: new Set(),
    anchor: null,
    rangeOrigin: null,
    rangeBase: new Set(),
    busy: false,
    message: null,
    pendingAction: null,
    failure: null,
  };
}
