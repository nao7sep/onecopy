// @vitest-environment happy-dom

import { EMPTY_ITEM_WORK } from "../helpers/itemWork";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  useComparisonStore,
  type GroupMember,
} from "../../src/state/comparison-store";
import {
  emitCalls,
  mockCommands,
  resetTauriMocks,
} from "../mocks/tauri";
import {
  reconcileComparisonMembership,
} from "../../src/workflows/comparison";
import { useItemsStore } from "../../src/state/items-store";
import { useSectionsStore } from "../../src/state/sections-store";
import { useIssuesStore } from "../../src/state/issues-store";
import type { SectionItem } from "../../src/models/items";
import { usePreviewStore } from "../../src/state/preview-store";

function member(index: number): GroupMember {
  return {
    hash: `h${index}`,
    fileName: `image-${index}.jpg`,
    width: 4000,
    height: 3000,
    byteSize: 1000,
    sharpness: null,
    faceScore: null,
    copyCount: 1,
    hasThumb: true,
  };
}

function openSession(count = 5): void {
  const members = Array.from({ length: count }, (_, index) => member(index));
  useComparisonStore.setState({
    sessionId: 0,
    open: true,
    members,
    originalMemberHashes: members.map((item) => item.hash),
    page: 0,
    maximumImages: 16,
    displayCount: 1,
    displayAspects: [16 / 9],
    capacities: [4],
    portraitDominant: false,
    spreadCount: 0,
    selected: new Set(["h1", "h2"]),
    anchors: new Set(["h2"]),
    anchor: "h2",
    rangeOrigin: "h2",
    rangeBase: new Set(["h1", "h2"]),
    busy: false,
    message: null,
    pendingAction: null,
    failure: null,
  });
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    set_window_fullscreen: () => null,
    set_spread_fullscreen: () => null,
  });
  useItemsStore.setState({ refresh: vi.fn(async () => undefined) });
  useSectionsStore.setState({ loadCounts: vi.fn(async () => undefined) });
  useIssuesStore.setState({ load: vi.fn(async () => undefined) });
  usePreviewStore.setState({ follow: false, placement: null, current: null });
  openSession();
});

describe("live membership reconciliation", () => {
  it("removes missing members but never admits a newly discovered image", async () => {
    await useComparisonStore
      .getState()
      .reconcileLiveMembers(["h0", "h2", "h4", "new"]);
    expect(
      useComparisonStore.getState().members.map((item) => item.hash),
    ).toEqual(["h0", "h2", "h4"]);
  });

  it("recovers a missing anchor to a surviving selected image", async () => {
    await useComparisonStore
      .getState()
      .reconcileLiveMembers(["h0", "h1", "h3", "h4"]);
    expect(useComparisonStore.getState().anchor).toBe("h1");
    expect(useComparisonStore.getState().selected.has("h1")).toBe(true);
  });

  it("does not apply a delayed refresh to a newer session", async () => {
    let answer: (hashes: string[]) => void = () => undefined;
    mockCommands({
      comparison_live_hashes: () =>
        new Promise<string[]>((resolve) => {
          answer = resolve;
        }),
    });
    useComparisonStore.setState({ sessionId: 1 });
    const refresh = reconcileComparisonMembership();
    const replacement = [member(8), member(9)];
    useComparisonStore.setState({
      sessionId: 2,
      members: replacement,
      originalMemberHashes: replacement.map((item) => item.hash),
    });
    answer([]);
    await refresh;

    expect(useComparisonStore.getState().members).toEqual(replacement);
  });
});

function derived(hash: string): SectionItem {
  return { ...member(2), hash, faceScore: 0.8, sharpness: 45, pathId: 2,
    resolvedUtcMs: null, similarGroupId: null, hasCompanions: false,
    durationMs: null, dirPaths: [], derivedWork: EMPTY_ITEM_WORK };
}

it("refreshes comparison facts and its secondary projection without changing the draft", () => {
  const before = useComparisonStore.getState();
  useComparisonStore.getState().applyDerivedItem("h2", derived("h2"));
  const after = useComparisonStore.getState();
  expect(after.members.map((item) => item.hash)).toEqual(before.members.map((item) => item.hash));
  expect(after.members[2]).toMatchObject({ faceScore: 0.8, sharpness: 45 });
  expect(after.selected).toBe(before.selected);
  expect(after.anchors).toBe(before.anchors);
  expect(after.anchor).toBe(before.anchor);
  expect(after.page).toBe(before.page);
  const latest = emitCalls.filter((call) => call.event === "comparison://state").at(-1)?.payload;
  expect(latest).toMatchObject({ chunks: [expect.arrayContaining([
    expect.objectContaining({ member: expect.objectContaining({ hash: "h2", faceScore: 0.8 }) }),
  ])] });
});

it("carries comparison identity promotion through marks and pending intent", () => {
  useComparisonStore.setState({ pendingAction: { kind: "selection", permanent: false, keepHashes: ["h2"], targetHashes: ["h1"] } });
  useComparisonStore.getState().applyDerivedItem("h2", derived("established"));
  const state = useComparisonStore.getState();
  expect(state.members.map((item) => item.hash)).toEqual(["h0", "h1", "established", "h3", "h4"]);
  expect(state.originalMemberHashes).toContain("established");
  expect(state.selected).toEqual(new Set(["h1", "established"]));
  expect(state.anchors).toEqual(new Set(["established"]));
  expect(state.anchor).toBe("established");
  expect(state.rangeOrigin).toBe("established");
  expect(state.pendingAction?.keepHashes).toEqual(["established"]);
});

it("never reopens a closed comparison or inserts a result into a replacement session", () => {
  useComparisonStore.setState({ open: false });
  const closed = useComparisonStore.getState();
  closed.applyDerivedItem("h2", derived("h2"));
  expect(useComparisonStore.getState()).toBe(closed);
  openSession(2);
  const replacement = useComparisonStore.getState();
  replacement.applyDerivedItem("h2", derived("h2"));
  expect(useComparisonStore.getState()).toBe(replacement);
});
