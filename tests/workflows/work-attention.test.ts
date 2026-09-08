// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installWorkAttention, setWorkViewport } from "../../src/workflows/work-attention";
import { useItemsStore } from "../../src/state/items-store";
import { useComparisonStore } from "../../src/state/comparison-store";
import { useQuickViewStore } from "../../src/state/quick-view-store";
import { EMPTY_ITEM_WORK } from "../../src/models/items";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  vi.useFakeTimers();
  resetTauriMocks();
  mockCommands({ prioritize_derived_work: () => null });
  useItemsStore.setState({ selected: { kind: "video", month: "2026-09" }, selectedItem: "video", totalItems: 1000 });
  useComparisonStore.setState({ open: false });
  useQuickViewStore.setState({ session: null });
  installWorkAttention();
});
afterEach(async () => { await vi.advanceTimersByTimeAsync(100); vi.useRealTimers(); });

describe("shared work attention workflow", () => {
  it("coalesces scrolling, follows Comparison across displays, and restores Main", async () => {
    for (let index = 0; index < 20; index++) setWorkViewport({
      sectionKey: "video:2026-09", visibleHashes: [`visible-${index}`], nearbyHashes: [`near-${index}`], anchor: index,
    });
    await vi.advanceTimersByTimeAsync(100);
    const main = invokeCalls.filter((call) => call.command === "prioritize_derived_work");
    expect(main).toHaveLength(1);
    expect(main[0].args).toMatchObject({ visibleHashes: ["visible-19"], nearbyHashes: ["near-19"], sectionAnchor: 19, sectionTotal: 1000 });

    const members = ["left", "right", "later"].map((hash) => ({
      hash, fileName: hash, byteSize: 1, width: 100, height: 100, sharpness: null,
      faceScore: null, copyCount: 1, hasThumb: true, resolvedUtcMs: null, derivedWork: EMPTY_ITEM_WORK,
    }));
    useComparisonStore.setState({ open: true, members, anchor: "right", page: 0, maximumImages: 2, displayCount: 2, capacities: [1, 1] });
    await vi.advanceTimersByTimeAsync(100);
    const comparison = invokeCalls.filter((call) => call.command === "prioritize_derived_work").at(-1)!;
    expect(comparison.args).toMatchObject({ selectedHash: "right", visibleHashes: ["left", "right"], nearbyHashes: [], sectionKind: null });
    expect(Number(comparison.args.generation)).toBeGreaterThan(Number(main[0].args.generation));

    useComparisonStore.setState({ open: false });
    await vi.advanceTimersByTimeAsync(100);
    expect(invokeCalls.filter((call) => call.command === "prioritize_derived_work").at(-1)?.args).toMatchObject({ selectedHash: "video", visibleHashes: ["visible-19"], sectionKind: "video" });
  });
});
