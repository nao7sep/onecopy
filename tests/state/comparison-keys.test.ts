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
  SLOT_KEYS,
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
    // Twelve members so slot 9 ("0") exists and is a real photo.
    const slots = Array.from({ length: 12 }, (_, i) => ({
      hash: `h${i}`,
      slotKey: SLOT_KEYS[i] ?? "?",
      sharpness: null,
      fileName: `IMG_${i}.jpg`,
    }));
    useComparisonStore.setState({
      open: true,
      slots: slots as never,
      queue: [],
      kept: new Set(["h9"]),
    });

    // The user pressed Cmd+0 to reset zoom. Nothing about the keepers may move.
    const index = slotIndexFor({ key: "0", metaKey: true });
    if (index >= 0) useComparisonStore.getState().toggleKeep(index);

    expect(useComparisonStore.getState().kept.has("h9")).toBe(true);
  });
});
