// The unlink and the chain-to-next-family flow (the developer's cull rhythm,
// 2026-08-17): spot an intruder in a turn → Shift+its key removes it from the
// set (never deletes, and the verdict persists core-side); its slot stays as
// a HOLE so the other keys keep their numbers; and when the family is fully
// decided, the grid anchor lands on the next photo past it so Enter chains
// straight into the next group.

import { beforeEach, describe, expect, it } from "vitest";
import {
  SLOT_KEYS,
  slotIndexForShiftedCode,
  chunkSlots,
  liveSlotCount,
  screensNeeded,
  useComparisonStore,
} from "../../src/state/comparison-store";
import { useItemsStore } from "../../src/state/items-store";
import type { SectionItem } from "../../src/models/items";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

function member(i: number) {
  return {
    hash: `h${i}`,
    fileName: `IMG_${i}.jpg`,
    width: 100,
    height: 100,
    byteSize: 1000,
    sharpness: null,
    faceScore: null,
    copyCount: 1,
    hasThumb: true,
  };
}

function openSession(count: number): void {
  const members = Array.from({ length: count }, (_, i) => member(i));
  useComparisonStore.setState({
    open: true,
    members,
    kept: new Set(),
    visited: new Set([0]),
    page: 0,
    shortlist: false,
    shortlistPage: 0,
    sessionMembers: members.map((m) => m.hash),
    capacities: [count],
    busy: false,
    permanentArmed: true,
    pendingPermanentCommit: false,
    pendingCommit: null,
  });
}

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    similar_unlink: () => 3,
    delete_item: () => ({ deletedFiles: 1, failedFiles: 0, removedRows: 1 }),
    get_section_items: () => [],
    get_section_counts: () => [],
    patch_state: () => ({}),
    get_item_detail: () => null,
  });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-01" },
    items: [],
    selectedItem: null,
    selectedKeys: new Set(),
    sortOrders: { media: { order: "time", desc: false }, other: { order: "name", desc: false } },
  });
});

describe("the shifted slot chord", () => {
  it("resolves physical digit and letter codes, layout-independently", () => {
    // Shift+1 delivers `key: "!"` on US layouts and other symbols elsewhere;
    // only `code` names the physical key every layout shares.
    expect(slotIndexForShiftedCode({ code: "Digit1", shiftKey: true })).toBe(0);
    expect(slotIndexForShiftedCode({ code: "Digit9", shiftKey: true })).toBe(8);
    expect(slotIndexForShiftedCode({ code: "Digit0", shiftKey: true })).toBe(9);
    expect(slotIndexForShiftedCode({ code: "KeyA", shiftKey: true })).toBe(10);
    expect(slotIndexForShiftedCode({ code: "KeyF", shiftKey: true })).toBe(15);
  });

  it("refuses everything that is not the bare Shift chord", () => {
    expect(slotIndexForShiftedCode({ code: "Digit1", shiftKey: false })).toBe(-1);
    expect(slotIndexForShiftedCode({ code: "KeyG", shiftKey: true })).toBe(-1);
    expect(
      slotIndexForShiftedCode({ code: "Digit1", shiftKey: true, ctrlKey: true }),
    ).toBe(-1);
    expect(
      slotIndexForShiftedCode({ code: "Digit1", shiftKey: true, altKey: true }),
    ).toBe(-1);
  });
});

describe("unlinking a slot", () => {
  it("records the verdict, leaves a hole, and keeps every other key number", async () => {
    openSession(4);
    useComparisonStore.getState().toggleKeep(1); // the intruder was even kept

    await useComparisonStore.getState().unlinkSlot(1);

    const after = useComparisonStore.getState();
    expect(
      invokeCalls.filter((c) => c.command === "similar_unlink").map((c) => c.args.hash),
    ).toEqual(["h1"]);
    expect(after.members[1]).toBeNull();
    expect(after.kept.has("h1")).toBe(false);
    // The photo is out of the family, so the finish may land the anchor on it.
    expect(after.sessionMembers).not.toContain("h1");
    // The hole PRESERVES key numbers: slot 3 is still key "3"... spelled as
    // the chunk the windows render.
    const chunk = chunkSlots(after.members, after.kept, [4])[0]!;
    expect(chunk[1]!.member).toBeNull();
    expect(chunk[2]!.member?.hash).toBe("h2");
    expect(chunk[2]!.slotKey).toBe(SLOT_KEYS[2]);
  });

  it("stops counting a hole as a photo on screen", async () => {
    // The header reads "N shown"; after an unlink the array still has four
    // entries but only three photos.
    openSession(4);
    await useComparisonStore.getState().unlinkSlot(1);
    expect(liveSlotCount(useComparisonStore.getState().members)).toBe(3);
  });

  it("never deletes the unlinked photo on commit", async () => {
    openSession(3);
    await useComparisonStore.getState().unlinkSlot(2); // h2 is not family
    useComparisonStore.getState().toggleKeep(0); // keep h0

    await useComparisonStore.getState().commitTurn(false);

    const deleted = invokeCalls
      .filter((c) => c.command === "delete_item")
      .map((c) => c.args.hash);
    // Only the unkept FAMILY member dies — never the unlinked photo.
    expect(deleted).toEqual(["h1"]);
  });
});

describe("finishing a family", () => {
  function gridItem(i: number, over: Partial<SectionItem> = {}): SectionItem {
    return {
      hash: `h${i}`,
      pathId: i,
      fileName: `IMG_${i}.jpg`,
      resolvedUtcMs: i * 1000,
      copyCount: 1,
      width: 100,
      height: 100,
      hasThumb: true,
      similarGroupId: null,
      sharpness: null,
      byteSize: 1000,
      hasCompanions: false,
      durationMs: null,
      ...over,
    };
  }

  it("lands the anchor on the first photo PAST the family", async () => {
    // Grid after the commit's refresh: keeper h0, then the next family at h5.
    mockCommands({
      get_section_items: () => [gridItem(0), gridItem(5), gridItem(6)],
    });
    useItemsStore.setState({ items: [gridItem(0), gridItem(5), gridItem(6)] });
    openSession(3); // h0..h2, no queue — this commit finishes the family
    useComparisonStore.getState().toggleKeep(0);

    await useComparisonStore.getState().commitTurn(false);

    expect(useComparisonStore.getState().open).toBe(false);
    // Past the keeper — Enter on h0 would reopen the family just decided.
    expect(useItemsStore.getState().selectedItem).toBe("h5");
  });

  it("rests on the last keeper when the family sat at the end", async () => {
    mockCommands({ get_section_items: () => [gridItem(0)] });
    useItemsStore.setState({ items: [gridItem(0)] });
    openSession(3);
    useComparisonStore.getState().toggleKeep(0);

    await useComparisonStore.getState().commitTurn(false);

    expect(useItemsStore.getState().selectedItem).toBe("h0");
  });
});

describe("how many screens a family fills", () => {
  // A spread window with nothing to show must not open: an empty
  // always-on-top surface covering a monitor is a curtain, not a comparison
  // aid — the developer's 6-member family on 3 screens got a third window
  // showing nothing, every time.
  it("opens only the screens the members fill", () => {
    expect(screensNeeded(6, 4, 3)).toBe(2); // 4 + 2, third screen dark
    expect(screensNeeded(12, 4, 3)).toBe(3); // exactly full
    expect(screensNeeded(13, 4, 3)).toBe(3); // overflow queues, never a 4th
    expect(screensNeeded(2, 4, 3)).toBe(1); // a pair never spreads at all
  });

  it("always keeps the main window's screen", () => {
    expect(screensNeeded(0, 4, 3)).toBe(1);
    expect(screensNeeded(1, 3, 1)).toBe(1);
  });
});
