import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  useComparisonStore,
  type GroupMember,
} from "../../src/state/comparison-store";
import {
  invokeCalls,
  mockCommands,
  resetTauriMocks,
  WebviewWindow,
} from "../mocks/tauri";
import {
  reconcileComparisonMembership,
  unlinkComparisonSelection,
} from "../../src/workflows/comparison";
import { useItemsStore } from "../../src/state/items-store";
import { useSectionsStore } from "../../src/state/sections-store";
import { useIssuesStore } from "../../src/state/issues-store";
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
    similar_unlink: () => 4,
    set_window_simple_fullscreen: () => null,
  });
  useItemsStore.setState({ refresh: vi.fn(async () => undefined) });
  useSectionsStore.setState({ loadCounts: vi.fn(async () => undefined) });
  useIssuesStore.setState({ load: vi.fn(async () => undefined) });
  usePreviewStore.setState({ follow: false, placement: null, current: null });
  openSession();
});

describe("Not similar", () => {
  it("records and removes every selected image without leaving holes", async () => {
    await useComparisonStore.getState().unlinkSelected();

    expect(
      invokeCalls
        .filter((call) => call.command === "similar_unlink")
        .map((call) => call.args.hash),
    ).toEqual(["h1", "h2"]);
    expect(
      useComparisonStore.getState().members.map((item) => item.hash),
    ).toEqual(["h0", "h3", "h4"]);
    expect(useComparisonStore.getState().selected).toEqual(new Set(["h3"]));
  });

  it("keeps failed selected images and explains the partial result", async () => {
    mockCommands({
      similar_unlink: ({ hash }) => {
        if (hash === "h2") throw new Error("write failed");
        return 4;
      },
    });

    await useComparisonStore.getState().unlinkSelected();

    expect(
      useComparisonStore.getState().members.map((item) => item.hash),
    ).toEqual(["h0", "h2", "h3", "h4"]);
    expect(useComparisonStore.getState().message).toContain(
      "1 image could not",
    );
  });

  it("closes when fewer than two comparable images remain", async () => {
    openSession(2);
    useComparisonStore.setState({
      selected: new Set(["h0"]),
      anchors: new Set(["h0"]),
      anchor: "h0",
    });
    await useComparisonStore.getState().unlinkSelected();
    expect(useComparisonStore.getState().open).toBe(false);
  });

  it("restores a persistent Preview window after an automatic close", async () => {
    const preview = new WebviewWindow("preview");
    usePreviewStore.setState({ follow: true, placement: "window" });
    openSession(2);
    useComparisonStore.setState({
      selected: new Set(["h0"]),
      anchors: new Set(["h0"]),
      anchor: "h0",
    });

    await unlinkComparisonSelection();

    expect(preview.show).toHaveBeenCalledOnce();
  });
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
