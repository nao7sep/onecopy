// @vitest-environment happy-dom

import { EMPTY_ITEM_WORK } from "../helpers/itemWork";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { beginMainFeedback, currentMainFeedback, invalidateMainFeedback, useMainFeedbackStore } from "../../src/state/main-feedback-store";
import { useItemsStore } from "../../src/state/items-store";
import { useComparisonStore } from "../../src/state/comparison-store";
import { useNotificationsStore } from "../../src/state/notifications-store";
import { requestComparisonFromMain } from "../../src/workflows/comparison";
import { openFullscreenView } from "../../src/workflows/fullscreen-view";
import type { SectionItem } from "../../src/models/items";
import { invokeCalls, mockCommands, mockSectionItems, resetTauriMocks } from "../mocks/tauri";
import { message } from "../../src/i18n/translate";
import { inEnglish } from "../helpers/i18n";

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
    expect(current()?.tone).toBe("normal");
    expect(inEnglish(current()?.text)).toBe(
      "Comparison requires images from one similar group.",
    );
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

  it("leaves whether a selection is one similar group to the core", async () => {
    const asked: unknown[] = [];
    mockCommands({ comparison_selection_valid: (args) => { asked.push(args.hashes); return false; } });
    useItemsStore.getState().selectItem("h1", "nearest", 0);
    await requestComparisonFromMain();
    expect(asked).toEqual([["h1"]]);
    expect(inEnglish(current()?.text)).toBe(
      "Comparison requires images from one similar group.",
    );
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
      beginMainFeedback("comparison").finish({
        tone: "normal",
        text: message("comparison.needsOneGroup"),
      });
      await action();
      expect(current()?.text.key).not.toBe("comparison.needsOneGroup");
    }
  });

  it("does not clear an unchanged item's failure merely because the section is sorted", async () => {
    beginMainFeedback("detail").finish({
      tone: "danger",
      text: message("item.detailLoadFailed"),
    });
    useItemsStore.getState().setSortOrder("name");
    await useItemsStore.getState().refresh();
    expect(current()?.text.key).toBe("item.detailLoadFailed");
  });

  it.each(["full", "window", "derived"] as const)("updates detail through a %s background read without clearing command feedback", async (refresh) => {
    beginMainFeedback("detail").finish({ tone: "danger", text: message("item.detailLoadFailed") });
    const previous = current();
    let resolveDetail!: (detail: unknown) => void;
    mockCommands({ get_item_detail: () => new Promise((resolve) => { resolveDetail = resolve; }) });
    const changed = { ...rows[0], copyCount: 2 };
    if (refresh === "full") await useItemsStore.getState().refresh();
    else if (refresh === "window") {
      mockSectionItems(() => [changed, rows[1]]);
      await useItemsStore.getState().refreshWindow();
    } else useItemsStore.getState().applyDerivedItem("h1", changed);
    expect(current()).toBe(previous);
    resolveDetail({ fileName: "refreshed.jpg", copyCount: 2, copyPaths: ["/fixture/a.jpg", "/backup/a.jpg"] });
    await vi.waitFor(() => expect(useItemsStore.getState().detail?.fileName).toBe("refreshed.jpg"));
    expect(useItemsStore.getState().detail?.copyCount).toBe(2);
    expect(current()).toBe(previous);
  });

  it("records a failed background detail read without replacing an existing Main command result", async () => {
    beginMainFeedback("detail").finish({ tone: "danger", text: message("item.detailLoadFailed") });
    const previous = current();
    mockCommands({
      get_item_detail: () => { throw new Error("background detail read failed"); },
      log_event: () => null,
    });
    await useItemsStore.getState().refresh();
    await vi.waitFor(() => expect(invokeCalls.filter((call) => call.command === "record_recent_notification" &&
      (call.args.request as { kind?: string }).kind === "item-detail-load-failed")).toHaveLength(1));
    expect(current()).toBe(previous);
  });

  it("keeps foreground selection detail failures in Main command feedback", async () => {
    mockCommands({
      get_item_detail: () => { throw new Error("selected detail read failed"); },
      log_event: () => null,
    });
    useItemsStore.getState().selectItem("h2", "nearest", 1);
    await vi.waitFor(() => expect(current()?.text.key).toBe("item.detailLoadFailed"));
  });

  it("keeps section failure across item navigation, but not a section transition", async () => {
    beginMainFeedback("recheck").finish({
      tone: "danger",
      text: message("section.rescanFailed"),
    });
    useItemsStore.getState().selectItem("h2", "nearest", 1);
    await requestComparisonFromMain();
    expect(inEnglish(current()?.text)).toBe("This section could not be refreshed. Try again.");
    await useItemsStore.getState().select({ ...SECTION, month: "2026-02" });
    expect(current()).toBeNull();
  });

  it("supersedes only the same command and rejects callbacks after context invalidation", () => {
    const old = beginMainFeedback("fullscreen-view");
    const latest = beginMainFeedback("fullscreen-view");
    latest.finish({ tone: "normal", text: message("fullscreenView.selectItemFirst") });
    old.finish({ tone: "danger", text: message("fullscreenView.openFailed") });
    expect(current()?.text.key).toBe("fullscreenView.selectItemFirst");
    invalidateMainFeedback("selection");
    latest.finish({ tone: "danger", text: message("fullscreenView.navigationFailed") });
    expect(current()).toBeNull();
  });

  it("does not erase independent persistent failures when Main feedback changes", async () => {
    const failure = {
      id: 7, kind: "worker-failed", path: null, level: "error" as const,
      presentation: "persistent" as const, message: "Background worker stopped.",
      messageKey: null, messageValues: null,
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
    expect(openFullscreenView()).toBe(false);
    expect(current()?.tone).toBe("normal");
    expect(inEnglish(current()?.text)).toBe("Select an item to open the fullscreen view.");
    useItemsStore.getState().selectItem("h1", "nearest", 0);
    expect(current()).toBeNull();
  });
});
