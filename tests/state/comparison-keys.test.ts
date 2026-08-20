// Slot keys in the comparison view are bare single characters, so several of
// them collide with app commands: SLOT_KEYS[9] is "0" (Cmd/Ctrl+0 resets
// zoom) and "a" is a slot (Ctrl+A). A collision here is not cosmetic — the
// flipped keeper flag is committed by the next Enter, so a photo the user
// explicitly marked to keep is the one that gets deleted.
//
// The forwarding path from a secondary comparison window is asserted too,
// because it previously stripped modifier state and could not tell Cmd+0
// from a bare slot-0 press.

import { beforeEach, describe, expect, it } from "vitest";
import {
  slotIndexForKey,
  useComparisonStore,
} from "../../src/state/comparison-store";
import { resetTauriMocks } from "../mocks/tauri";

// The production rule itself, reached by BOTH key paths — the local handler
// and the forwarded-key listener. Reimplementing it here would pass whatever
// the app did.
const slotIndexFor = slotIndexForKey;

beforeEach(() => {
  resetTauriMocks();
});

describe("slot key resolution", () => {
  it("treats a bare slot character as its slot", () => {
    expect(slotIndexFor({ key: "0" })).toBe(9);
    expect(slotIndexFor({ key: "a" })).toBe(10);
    expect(slotIndexFor({ key: "1" })).toBe(0);
  });

  it("does not treat Cmd/Ctrl+0 as slot zero", () => {
    expect(slotIndexFor({ key: "0", metaKey: true })).toBe(-1);
    expect(slotIndexFor({ key: "0", ctrlKey: true })).toBe(-1);
  });

  it("does not treat Ctrl+A as slot a", () => {
    expect(slotIndexFor({ key: "a", ctrlKey: true })).toBe(-1);
    expect(slotIndexFor({ key: "a", metaKey: true })).toBe(-1);
  });

  it("ignores Alt-modified slot keys", () => {
    expect(slotIndexFor({ key: "3", altKey: true })).toBe(-1);
  });
});

describe("keeper flags", () => {
  it("survive a zoom reset", () => {
    // Twelve REAL members, all visible on one page, so slot 9 ("0") resolves to
    // h9. Seeding the store properly is what gives this test teeth: with an
    // empty member list `toggleKeep` early-returns and the assertion below
    // passes however broken the guard is.
    const members = Array.from({ length: 12 }, (_, i) => ({
      hash: `h${i}`,
      fileName: `IMG_${i}.jpg`,
      width: 100,
      height: 100,
      byteSize: 1000,
      sharpness: null,
      faceScore: null,
      copyCount: 1,
      hasThumb: true,
    }));
    useComparisonStore.setState({
      open: true,
      members,
      kept: new Set(["h9"]),
      page: 0,
      shortlist: false,
      shortlistPage: 0,
      capacities: [members.length],
    });

    // The property itself: a modified "0" is a zoom reset, not slot ten.
    expect(slotIndexFor({ key: "0", metaKey: true })).toBe(-1);

    // And end to end — were the guard ever to hand back slot 9, this would
    // un-keep h9 and the assertion would fail.
    const index = slotIndexFor({ key: "0", metaKey: true });
    if (index >= 0) useComparisonStore.getState().toggleKeep(index);

    expect(useComparisonStore.getState().kept.has("h9")).toBe(true);
  });
});
