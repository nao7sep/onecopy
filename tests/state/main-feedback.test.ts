// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { beginMainFeedback, currentMainFeedback, invalidateMainFeedback, useMainFeedbackStore } from "../../src/state/main-feedback-store";
import { useItemsStore } from "../../src/state/items-store";
import { useComparisonStore } from "../../src/state/comparison-store";
import { useNotificationsStore } from "../../src/state/notifications-store";
import { requestComparisonFromMain } from "../../src/workflows/comparison";
import { openViewerFromMain } from "../../src/workflows/quick-view";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import { mockCommands, mockSectionItems, resetTauriMocks } from "../mocks/tauri";

const SECTION = { kind: "image" as const, month: "2026-01" };
const rows: SectionItem[] = [1, 2].map((pathId) => ({
  hash: `h${pathId}`, pathId, fileName: `fixture-${pathId}.jpg`, resolvedUtcMs: pathId,
  copyCount: 1, width: 100, height: 100, hasThumb: true, similarGroupId: null,
  similarCount: 0, sharpness: null, faceScore: null, byteSize: 100, hasCompanions: false,
  durationMs: null, dirPaths: ["/fixture"], derivedWork: EMPTY_ITEM_WORK,
}));
const current = () => currentMainFeedback(useMainFeedbackStore.getState());

beforeEach(async () => {
  resetTauriMocks();
  useMainFeedbackStore.setState({ entries: {} });
  useItemsStore.setState(useItemsStore.getInitialState(), true);
  useComparisonStore.setState({ open: false, originalMemberHashes: [] });
  useNotificationsStore.setState({ active: [] });
  mockSectionItems(() => rows);
  mockCommands({
    get_item_detail: () => null,
    get_section_family_context: () => null,
    comparison_selection_valid: () => false,
    record_recent_notification: () => null,
  });
  await useItemsStore.getState().select(SECTION);
});
afterEach(() => { vi.restoreAllMocks(); });

describe("Main command feedback ownership", () => {
  it("clears informational Comparison refusal on another selection and on a successful same-context retry", async () => {
    await requestComparisonFromMain();
    expect(current()).toEqual({ tone: "normal", text: "Comparison requires images from one similar group." });
    useItemsStore.getState().selectItem("h2", "nearest", 1);
    expect(current()).toBeNull();
    await requestComparisonFromMain();
    expect(current()?.tone).toBe("normal");

    mockCommands({ comparison_selection_valid: () => true });
    const open = vi.spyOn(useComparisonStore.getState(), "openGroup").mockResolvedValue("opened");
    await requestComparisonFromMain();
    expect(open).toHaveBeenCalledOnce();
    expect(current()).toBeNull();
  });

  it("cannot publish a late admission result or open an obsolete selection", async () => {
    let reply!: (valid: boolean) => void;
    mockCommands({ comparison_selection_valid: () => new Promise<boolean>((resolve) => { reply = resolve; }) });
    const open = vi.spyOn(useComparisonStore.getState(), "openGroup").mockResolvedValue("opened");
    const request = requestComparisonFromMain();
    useItemsStore.getState().selectItem("h2", "nearest", 1);
    reply(true);
    await request;
    expect(current()).toBeNull();
    expect(open).not.toHaveBeenCalled();
  });

  it("keeps unchanged background reconciliation from clearing command feedback", async () => {
    await requestComparisonFromMain();
    const before = current();
    await useItemsStore.getState().refresh();
    expect(current()).toBe(before);
  });

  it("invalidates selection feedback through range, toggle, anchor, and identity commands", async () => {
    const actions = [
      () => useItemsStore.getState().rangeSelect("h2", 1),
      () => useItemsStore.getState().toggleItem("h1", 0),
      () => useItemsStore.getState().setAnchor("h2", 1),
      () => useItemsStore.getState().selectIdentity("h1"),
    ];
    for (const action of actions) {
      beginMainFeedback("comparison").finish({ tone: "normal", text: "Old selection guidance" });
      await action();
      expect(current()?.text).not.toBe("Old selection guidance");
    }
  });

  it("does not clear an unchanged item's failure merely because the section is sorted", async () => {
    beginMainFeedback("detail").finish({ tone: "danger", text: "Item details unavailable" });
    useItemsStore.getState().setSortOrder("name");
    await useItemsStore.getState().refresh();
    expect(current()?.text).toBe("Item details unavailable");
  });

  it("keeps section failure across item navigation, but not a section transition", async () => {
    beginMainFeedback("recheck").finish({ tone: "danger", text: "Couldn’t refresh this section." });
    useItemsStore.getState().selectItem("h2", "nearest", 1);
    await requestComparisonFromMain();
    expect(current()?.text).toBe("Couldn’t refresh this section.");
    await useItemsStore.getState().select({ ...SECTION, month: "2026-02" });
    expect(current()).toBeNull();
  });

  it("supersedes only the same command and rejects callbacks after context invalidation", () => {
    const old = beginMainFeedback("viewer");
    const latest = beginMainFeedback("viewer");
    latest.finish({ tone: "normal", text: "Latest guidance" });
    old.finish({ tone: "danger", text: "Obsolete failure" });
    expect(current()?.text).toBe("Latest guidance");
    invalidateMainFeedback("selection");
    latest.finish({ tone: "danger", text: "Late failure" });
    expect(current()).toBeNull();
  });

  it("does not erase independent persistent failures when Main feedback changes", async () => {
    const failure = {
      id: 7, kind: "worker-failed", path: null, level: "error" as const,
      presentation: "persistent" as const, message: "Background worker stopped.",
      firstSeenUtc: "2026-09-09T00:00:00Z", lastSeenUtc: "2026-09-09T00:00:00Z", occurrenceCount: 1,
    };
    useNotificationsStore.setState({ active: [failure] });
    await requestComparisonFromMain();
    useItemsStore.getState().selectItem("h2", "nearest", 1);
    expect(current()).toBeNull();
    expect(useNotificationsStore.getState().active).toEqual([failure]);
  });

  it("makes no-selection viewer feedback informational and clears it on selection", () => {
    useItemsStore.getState().selectItem(null);
    expect(openViewerFromMain("quick")).toBe(false);
    expect(current()).toEqual({ tone: "normal", text: "Select an item to open the viewer." });
    useItemsStore.getState().selectItem("h1", "nearest", 0);
    expect(current()).toBeNull();
  });
});
