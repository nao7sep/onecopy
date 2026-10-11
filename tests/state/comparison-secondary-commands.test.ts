// @vitest-environment happy-dom

import { beforeEach, expect, it, vi } from "vitest";
import { broadcastComparison, useComparisonStore, type ComparisonBroadcast, type GroupMember } from "../../src/state/comparison-store";
import { useItemsStore } from "../../src/state/items-store";
import { useSectionsStore } from "../../src/state/sections-store";
import { useIssuesStore } from "../../src/state/issues-store";
import { useAppStore } from "../../src/state/app-store";
import { restorePreviewAfterComparison, usePreviewStore } from "../../src/state/preview-store";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { closeComparison, decideComparisonPage, installComparisonEventWiring, openComparison, requestComparisonFromMain } from "../../src/workflows/comparison";
import { createdWindows, emitCalls, fireEvent, invokeCalls, mockCommands, resetTauriMocks, setCurrentMonitor, setMonitors } from "../mocks/tauri";

function members(count = 8): GroupMember[] {
  return Array.from({ length: count }, (_, index) => ({
    hash: `h${index}`, fileName: `image-${index}.jpg`, width: 4000, height: 3000,
    byteSize: 1, sharpness: null, faceScore: null, copyCount: 1, hasThumb: true,
  }));
}

const selectAfterFamily = useItemsStore.getState().selectAfterFamily;

function renderedProjection(): ComparisonBroadcast {
  broadcastComparison();
  return emitCalls.filter((call) => call.event === "comparison://state").at(-1)!.payload as ComparisonBroadcast;
}

beforeEach(async () => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    set_window_fullscreen: () => null, set_spread_fullscreen: () => null,
    get_similar_group: () => members(), get_section_family_context: () => null,
    comparison_live_hashes: () => useComparisonStore.getState().members.map((member) => member.hash),
    log_event: () => null, record_interface_failure: () => null,
    delete_items: ({ items }) => ({ cancelled: false, error: null, failedFiles: 0,
      items: (items as Array<{ hash: string; pathId: null }>).map((item) => ({ item, failedFiles: 0 })) }),
  });
  useItemsStore.setState({ selected: null, refresh: vi.fn(async () => undefined), selectAfterFamily: vi.fn(async () => undefined) });
  useSectionsStore.setState({ loadCounts: vi.fn(async () => undefined) });
  useIssuesStore.setState({ load: vi.fn(async () => undefined) });
  usePreviewStore.setState({ follow: false, placement: null, current: null, error: null });
  useAppStore.setState({ appData: { config: { confirmTrashDelete: false } } as never });
  useComparisonStore.setState({
    sessionId: useComparisonStore.getState().sessionId + 1, open: true,
    members: members(), originalMemberHashes: members().map((member) => member.hash),
    page: 0, maximumImages: 4, displayCount: 1, displayAspects: [16 / 9],
    capacities: [4], portraitDominant: false, spreadCount: 0,
    selected: new Set(["h1"]), anchors: new Set(["h1"]), anchor: "h1",
    rangeOrigin: "h1", rangeBase: new Set(["h1"]), busy: false,
    message: null, pendingAction: null, failure: null,
  });
  await installComparisonEventWiring();
});

it.each(["membership", "page", "layout", "marks", "session"] as const)(
  "rejects a secondary card and decision keys after %s authority changes",
  async (change) => {
    const shown = renderedProjection();
    const store = useComparisonStore.getState();
    if (change === "membership") await store.reconcileLiveMembers(members().slice(1).map((member) => member.hash));
    else if (change === "page") store.nextPage();
    else if (change === "layout") store.setDisplayAspect(0, 9 / 16);
    else if (change === "marks") store.selectSlot(2, "toggle");
    else { await store.close(); await useComparisonStore.getState().openGroup("h0", "h0", 4); }
    // Settle the valid layout/page lifecycle before measuring stale input.
    await new Promise((resolve) => setTimeout(resolve, 0));
    const before = useComparisonStore.getState();
    const calls = invokeCalls.length;
    fireEvent("comparison://select", { sessionId: shown.sessionId, revision: shown.revision, slotIndex: 0, hash: "h0", mode: "toggle" });
    fireEvent("comparison://key", { sessionId: shown.sessionId, revision: shown.revision, key: "Delete", shiftKey: true });
    fireEvent("comparison://key", { sessionId: shown.sessionId, revision: shown.revision, key: "Enter", shiftKey: true });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useComparisonStore.getState().selected).toEqual(before.selected);
    expect(useComparisonStore.getState().pendingAction).toBeNull();
    expect(invokeCalls).toHaveLength(calls);
  },
);

it.each(["Delete", "Enter"])("rejects stale %s before deletion or page confirmation", async (key) => {
  const shown = renderedProjection();
  // Change authority directly before any later display effect/broadcast.
  useComparisonStore.setState({ selected: new Set(["h2"]) });
  invokeCalls.length = 0;
  fireEvent("comparison://key", { sessionId: shown.sessionId, revision: shown.revision, key });
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(invokeCalls).toEqual([]);
  expect(useComparisonStore.getState().pendingAction).toBeNull();
  expect(useComparisonStore.getState().selected).toEqual(new Set(["h2"]));
});

it("accepts current scoped cards and keys, requiring the exact clicked member", async () => {
  const shown = renderedProjection();
  fireEvent("comparison://select", { sessionId: shown.sessionId, revision: shown.revision, slotIndex: 2, hash: "wrong", mode: "toggle" });
  expect(useComparisonStore.getState().selected).toEqual(new Set(["h1"]));
  fireEvent("comparison://select", { sessionId: shown.sessionId, revision: shown.revision, slotIndex: 2, hash: "h2", mode: "toggle" });
  expect(useComparisonStore.getState().selected).toEqual(new Set(["h1", "h2"]));
  const current = renderedProjection();
  fireEvent("comparison://key", { sessionId: current.sessionId, revision: current.revision, key: "Enter" });
  expect(useComparisonStore.getState().pendingAction).toMatchObject({ kind: "page", keepHashes: ["h1", "h2"], targetHashes: ["h0", "h3"] });
});

it("rejects old-session geometry while accepting a current display measurement after marks change", async () => {
  const shown = renderedProjection();
  useComparisonStore.getState().selectSlot(2, "toggle");
  fireEvent("comparison://layout", { ...shown, slice: 0, aspect: 9 / 16 });
  expect(useComparisonStore.getState().displayAspects).toEqual([9 / 16]);
  fireEvent("comparison://layout", { sessionId: shown.sessionId - 1, revision: shown.revision, slice: 0, aspect: 1 });
  expect(useComparisonStore.getState().displayAspects).toEqual([9 / 16]);
  await new Promise((resolve) => setTimeout(resolve, 0));
});

it("still deletes the current single mark without a confirmation", async () => {
  const shown = renderedProjection();
  fireEvent("comparison://key", { sessionId: shown.sessionId, revision: shown.revision, key: "Delete" });
  await vi.waitFor(() => expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(true));
  expect(invokeCalls.find((call) => call.command === "delete_items")?.args.items).toEqual([{ hash: "h1", pathId: null }]);
  await vi.waitFor(() => expect(useComparisonStore.getState().busy).toBe(false));
  expect(useComparisonStore.getState().pendingAction).toBeNull();
});

it.each(["success", "failure"] as const)("keeps the newer session and return position after an older preparation %s", async (outcome) => {
  const section = { kind: "image" as const, month: "2026-01" };
  useItemsStore.setState({ selected: section });
  mockCommands({ get_similar_group: () => members(2) });
  let finishOld!: (value: unknown) => void;
  let failOld!: (error: Error) => void;
  mockCommands({ get_section_family_context: () => new Promise((resolve, reject) => { finishOld = resolve; failOld = reject; }) });
  const older = openComparison("h0");
  await vi.waitFor(() => expect(finishOld).toBeDefined());
  await closeComparison();
  const currentContext = { index: 8, before: [{ hash: "new-before", pathId: null }], after: [{ hash: "new-after", pathId: null }] };
  mockCommands({ get_section_family_context: () => currentContext });
  await openComparison("h0");
  const sessionId = useComparisonStore.getState().sessionId;
  if (outcome === "success") finishOld({ index: 1, before: [], after: [] });
  else failOld(new Error("obsolete context read failed"));
  await older;
  expect(useComparisonStore.getState()).toMatchObject({ open: true, sessionId });
  // Complete the current group to observe its owned return-position result.
  useComparisonStore.getState().markAll();
  await decideComparisonPage(false);
  expect(vi.mocked(useItemsStore.getState().selectAfterFamily).mock.calls[0]?.[0]).toEqual({ index: 8, before: ["new-before"], after: ["new-after"] });
});

it.each(["preparation", "window setup"] as const)("keeps the visible session's return position when a newer Main admission has no comparison during %s", async (stage) => {
  await closeComparison();
  useItemsStore.setState({ selected: { kind: "image", month: "2026-01" }, selectedItem: "h0", selectedKeys: new Set(["h0"]) });
  let validations = 0;
  let finishNewAdmission!: (value: boolean) => void;
  let finishOldGroup!: (value: GroupMember[]) => void;
  let finishContext!: (value: unknown) => void;
  let finishWindow!: () => void;
  let groups = 0;
  mockCommands({
    comparison_selection_valid: () => ++validations === 1 ? true : new Promise((resolve) => { finishNewAdmission = resolve; }),
    get_similar_group: () => ++groups === 1 ? new Promise((resolve) => { finishOldGroup = resolve; }) : members(1),
    get_section_family_context: () => new Promise((resolve) => { finishContext = resolve; }),
    set_window_fullscreen: ({ enable }) => stage === "window setup" && enable
      ? new Promise<void>((resolve) => { finishWindow = resolve; }) : null,
  });
  const older = requestComparisonFromMain();
  await vi.waitFor(() => expect(finishOldGroup).toBeDefined());
  // Both Main requests were admitted before Comparison opened. The second
  // validity read completes only after the first session is already visible.
  const newer = requestComparisonFromMain();
  await vi.waitFor(() => expect(finishNewAdmission).toBeDefined());
  finishOldGroup(members(2));
  await vi.waitFor(() => expect(stage === "preparation" ? finishContext : finishWindow).toBeDefined());
  const sessionId = useComparisonStore.getState().sessionId;
  finishNewAdmission(true);
  await newer;
  if (stage === "window setup") {
    finishWindow();
    await vi.waitFor(() => expect(finishContext).toBeDefined());
  }
  finishContext({ index: 8, before: [{ hash: "before", pathId: null }], after: [{ hash: "after", pathId: null }] });
  await older;
  expect(useComparisonStore.getState()).toMatchObject({ open: true, sessionId });
  useComparisonStore.getState().markAll();
  await decideComparisonPage(false);
  expect(vi.mocked(useItemsStore.getState().selectAfterFamily).mock.calls[0]?.[0]).toEqual({ index: 8, before: ["before"], after: ["after"] });
});

it("reports an admitted session's window failure after a newer Main attempt is unavailable", async () => {
  await closeComparison();
  invokeCalls.length = 0;
  const preview = new WebviewWindow("preview");
  usePreviewStore.setState({ follow: true, placement: "window" });
  useItemsStore.setState({ selected: { kind: "image", month: "2026-01" }, selectedItem: "h0", selectedKeys: new Set(["h0"]) });
  let validations = 0;
  let groups = 0;
  let finishNewAdmission!: (value: boolean) => void;
  let finishOldGroup!: (value: GroupMember[]) => void;
  let failWindow!: (error: Error) => void;
  mockCommands({
    comparison_selection_valid: () => ++validations === 1 ? true : new Promise((resolve) => { finishNewAdmission = resolve; }),
    get_similar_group: () => ++groups === 1 ? new Promise((resolve) => { finishOldGroup = resolve; }) : members(1),
    set_window_fullscreen: ({ enable }) => enable ? new Promise((_, reject) => { failWindow = reject; }) : null,
  });
  const older = requestComparisonFromMain();
  await vi.waitFor(() => expect(finishOldGroup).toBeDefined());
  const newer = requestComparisonFromMain();
  await vi.waitFor(() => expect(finishNewAdmission).toBeDefined());
  finishOldGroup(members(2));
  await vi.waitFor(() => expect(failWindow).toBeDefined());
  finishNewAdmission(true);
  await newer;
  const sessionId = useComparisonStore.getState().sessionId;
  failWindow(new Error("current fullscreen unavailable"));
  await older;
  expect(useComparisonStore.getState()).toMatchObject({ open: false, sessionId });
  expect(invokeCalls.filter((call) => call.command === "record_interface_failure").map((call) => call.args.messageKey)).toEqual(["comparison.openFailed"]);
  expect(invokeCalls.filter((call) => call.command === "set_window_fullscreen").map((call) => call.args.enable)).toEqual([true, false]);
  expect(preview.show).toHaveBeenCalledOnce();
  expect(vi.mocked(preview.setAlwaysOnTop).mock.calls).toEqual([[true], [false]]);
});

it.each(["closed", "replaced"] as const)("keeps the replacement session when the %s session's window setup fails", async (retirement) => {
  let failWindow!: (error: Error) => void;
  let entries = 0;
  mockCommands({ get_similar_group: () => members(2),
    set_window_fullscreen: ({ enable }) => enable && ++entries === 1
      ? new Promise((_, reject) => { failWindow = reject; }) : null });
  const older = openComparison("h0");
  await vi.waitFor(() => expect(failWindow).toBeDefined());
  const oldSessionId = useComparisonStore.getState().sessionId;
  const closing = retirement === "closed" ? closeComparison() : Promise.resolve();
  const newer = openComparison("h0");
  await new Promise((resolve) => setTimeout(resolve, 0));
  failWindow(new Error("obsolete fullscreen unavailable"));
  await Promise.all([older, closing, newer]);
  expect(useComparisonStore.getState().open).toBe(true);
  expect(useComparisonStore.getState().sessionId).toBeGreaterThan(oldSessionId);
  expect(invokeCalls.filter((call) => call.command === "record_interface_failure")).toEqual([]);
  expect(invokeCalls.filter((call) => call.command === "set_window_fullscreen").at(-1)?.args.enable).toBe(true);
});

it("opens the queued replacement's external display after the earlier session's window setup fails", async () => {
  const displays = [
    { name: "main", position: { x: 0, y: 0 }, size: { width: 2560, height: 1440 }, scaleFactor: 1 },
    { name: "external", position: { x: 2560, y: 0 }, size: { width: 2560, height: 1440 }, scaleFactor: 1 },
  ];
  setMonitors(displays);
  setCurrentMonitor(displays[0]!);
  useAppStore.setState({ appData: { config: { maximumImagesInComparison: 16 } } as never });
  let failWindow!: (error: Error) => void;
  let entries = 0;
  mockCommands({ get_similar_group: () => members(8),
    set_window_fullscreen: ({ enable }) => enable && ++entries === 1
      ? new Promise((_, reject) => { failWindow = reject; }) : null });
  const older = openComparison("h0");
  await vi.waitFor(() => expect(failWindow).toBeDefined());
  const newer = openComparison("h0");
  await new Promise((resolve) => setTimeout(resolve, 0));
  failWindow(new Error("obsolete fullscreen unavailable"));
  await Promise.all([older, newer]);
  expect(useComparisonStore.getState()).toMatchObject({ open: true, displayCount: 2, spreadCount: 1 });
  expect(createdWindows.find((window) => window.label === "comparison-1")?.options)
    .toMatchObject({ x: 2560, y: 0, width: 2560, height: 1440 });
  const external = await WebviewWindow.getByLabel("comparison-1");
  const created = vi.mocked(external!.once).mock.calls.find(([event]) => event === "tauri://created")![1];
  created({ event: "tauri://created", id: 1, payload: null });
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(external!.show).toHaveBeenCalledOnce();
  expect(invokeCalls.filter((call) => call.command === "set_spread_fullscreen").at(-1)?.args)
    .toEqual({ label: "comparison-1", enable: true });
  expect(invokeCalls.filter((call) => call.command === "set_window_fullscreen").at(-1)?.args.enable).toBe(true);
});

it.each(["success", "failure"] as const)("does not publish an older completed session's delayed Main recovery %s", async (outcome) => {
  useItemsStore.setState({ selected: { kind: "image", month: "2026-01" }, selectedItem: "h0", selectedKeys: new Set(["h0"]), selectAfterFamily, loadError: null });
  mockCommands({ get_similar_group: () => members(2), get_section_family_context: () => ({ index: 8, before: [], after: [] }),
    get_item_detail: () => ({ fileName: "recovered.jpg", kind: "image" }), record_recent_notification: () => ({}) });
  await openComparison("h0");
  let finishRecovery!: (value: unknown) => void;
  let failRecovery!: (error: Error) => void;
  mockCommands({ reconcile_section: () => new Promise((resolve, reject) => { finishRecovery = resolve; failRecovery = reject; }) });
  useComparisonStore.getState().markAll();
  const older = decideComparisonPage(false);
  await vi.waitFor(() => expect(finishRecovery).toBeDefined());
  await openComparison("h0");
  const sessionId = useComparisonStore.getState().sessionId;
  if (outcome === "success") finishRecovery({ anchor: { hash: "old-recovered", pathId: null, index: 8 }, selected: [{ hash: "old-recovered", pathId: null, index: 8 }],
    rangeOrigin: null, rangeBase: [], context: null, window: { start: 0, total: 0, items: [], order: "older" } });
  else failRecovery(new Error("obsolete Main recovery failed"));
  await older;
  expect(useComparisonStore.getState()).toMatchObject({ open: true, sessionId });
  expect(useItemsStore.getState()).toMatchObject({ selectedItem: "h0", loadError: null });
});

it.each(["success", "failure"] as const)("does not restore Preview after an older session's delayed lookup %s", async (outcome) => {
  const preview = new WebviewWindow("preview");
  usePreviewStore.setState({ follow: true, placement: "window" });
  mockCommands({ get_similar_group: () => members(2) });
  await openComparison("h0");
  let finishLookup!: (value: WebviewWindow | null) => void;
  let failLookup!: (error: Error) => void;
  vi.mocked(WebviewWindow.getByLabel).mockImplementationOnce(() => new Promise((resolve, reject) => { finishLookup = resolve; failLookup = reject; }));
  useComparisonStore.getState().markAll();
  const older = decideComparisonPage(false);
  await vi.waitFor(() => expect(finishLookup).toBeDefined());
  await openComparison("h0");
  const sessionId = useComparisonStore.getState().sessionId;
  if (outcome === "success") finishLookup(preview);
  else failLookup(new Error("obsolete Preview lookup failed"));
  await older;
  expect(useComparisonStore.getState()).toMatchObject({ open: true, sessionId });
  expect(preview.show).not.toHaveBeenCalled();
  expect(preview.setAlwaysOnTop).not.toHaveBeenCalled();
  expect(usePreviewStore.getState().error).toBeNull();
});

it("still reports current Main and Preview recovery failures", async () => {
  useItemsStore.setState({ selected: { kind: "image", month: "2026-01" }, selectAfterFamily, loadError: null });
  mockCommands({ reconcile_section: () => { throw new Error("current Main recovery failed"); }, record_recent_notification: () => ({}) });
  await selectAfterFamily({ index: 8, before: [], after: [] });
  expect(useItemsStore.getState().loadError?.key).toBe("section.loadFailed");
  usePreviewStore.setState({ follow: true, placement: "window" });
  vi.mocked(WebviewWindow.getByLabel).mockRejectedValueOnce(new Error("current Preview lookup failed"));
  await restorePreviewAfterComparison();
  expect(usePreviewStore.getState().error?.key).toBe("preview.restoreFailed");
});

it("always drops Preview's topmost pulse even after its Comparison session is replaced", async () => {
  const preview = new WebviewWindow("preview");
  usePreviewStore.setState({ follow: true, placement: "window" });
  mockCommands({ get_similar_group: () => members(2) });
  await openComparison("h0");
  let finishPulse!: () => void;
  vi.mocked(preview.setAlwaysOnTop).mockImplementationOnce(() => new Promise<void>((resolve) => { finishPulse = resolve; }));
  useComparisonStore.getState().markAll();
  const older = decideComparisonPage(false);
  await vi.waitFor(() => expect(finishPulse).toBeDefined());
  await openComparison("h0");
  finishPulse();
  await older;
  expect(vi.mocked(preview.setAlwaysOnTop).mock.calls).toEqual([[true], [false]]);
});
