// Turn mechanics for a group larger than the slot count.
//
// The queue is the only record of members not yet shown, so any path that
// drops it hides part of a group permanently — reopening the group refills
// the slots with the same keepers, leaving the tail unreachable.

import { beforeEach, describe, expect, it } from "vitest";
import { SLOT_KEYS, useComparisonStore } from "../../src/state/comparison-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

interface Member {
  hash: string;
  slotKey: string;
  sharpness: number | null;
  fileName: string;
}

function member(i: number): Member {
  return {
    hash: `h${i}`,
    slotKey: SLOT_KEYS[i] ?? "?",
    sharpness: null,
    fileName: `IMG_${i}.jpg`,
  };
}

/** A turn of `slotCount` members with `queued` still waiting behind it. */
function openTurn(slotCount: number, queued: number, capacities: number[]): void {
  const slots = Array.from({ length: slotCount }, (_, i) => member(i));
  const queue = Array.from({ length: queued }, (_, i) => member(slotCount + i));
  useComparisonStore.setState({
    open: true,
    slots: slots as never,
    queue: queue as never,
    kept: new Set(),
    capacities,
    busy: false,
    permanentArmed: true,
    pendingPermanentCommit: false,
  });
}

function deleted(): unknown[] {
  return invokeCalls
    .filter((c) => c.command === "delete_item")
    .map((c) => c.args.hash);
}

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    delete_item: () => ({ deletedFiles: 1, failedFiles: 0, removedRows: 1 }),
    get_section_items: () => [],
    get_section_counts: () => [],
    patch_state: () => ({}),
    get_item_detail: () => null,
  });
});

describe("committing a turn", () => {
  it("does not discard the queue when every slot is kept", async () => {
    // Two monitors: capacities [4,4] means a turn of 8. A 12-member burst
    // leaves 4 waiting; keeping all 8 must not lose them.
    openTurn(8, 4, [4, 4]);
    const store = useComparisonStore.getState();
    for (let i = 0; i < 8; i += 1) store.toggleKeep(i);

    await useComparisonStore.getState().commitTurn(false);

    const after = useComparisonStore.getState();
    expect(deleted()).toHaveLength(0);
    // Either the session stays open with the remaining members reachable, or
    // it closed having shown them — what it may never do is close with a
    // queue still holding photos the user never saw.
    expect(after.open && after.slots.length > 0).toBe(true);
    expect(after.slots.map((s) => s.hash)).toContain("h8");
  });

  it("closes when the queue really is empty", async () => {
    openTurn(4, 0, [4]);
    const store = useComparisonStore.getState();
    store.toggleKeep(0);

    await useComparisonStore.getState().commitTurn(false);

    expect(useComparisonStore.getState().open).toBe(false);
  });

  it("deletes exactly the slots that were not kept", async () => {
    openTurn(4, 0, [4]);
    useComparisonStore.getState().toggleKeep(2);

    await useComparisonStore.getState().commitTurn(false);

    expect(deleted().sort()).toEqual(["h0", "h1", "h3"]);
  });

  it("deletes nothing when no slot is kept", async () => {
    openTurn(4, 4, [4]);

    await useComparisonStore.getState().commitTurn(false);

    expect(deleted()).toHaveLength(0);
    // Undecided photos stay in the app; the turn simply advances.
    expect(useComparisonStore.getState().slots.map((s) => s.hash)).toEqual([
      "h4",
      "h5",
      "h6",
      "h7",
    ]);
  });

  it("stages a confirmation before the first permanent commit", async () => {
    openTurn(4, 0, [4]);
    useComparisonStore.setState({ permanentArmed: false });
    useComparisonStore.getState().toggleKeep(0);

    await useComparisonStore.getState().commitTurn(true);

    expect(deleted()).toHaveLength(0);
    expect(useComparisonStore.getState().pendingPermanentCommit).toBe(true);
  });
});
