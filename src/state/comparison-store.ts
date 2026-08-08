// The comparison view's turn machinery. A similar group opens into up to 16
// slots (keys 1–9, 0, a–f); slot keys toggle keepers; Enter commits the turn —
// non-kept slots are deleted (trash, or permanently with Shift), keepers stay
// pinned, and freed slots refill from the queue, which is exactly the
// "remaining photos coming in" the design asks for. Committing with no keeper
// skips the turn: those photos stay in the app, undecided, and the next batch
// flows in. The group is done when the queue is empty and every slot is kept
// (or skipped) — the view closes and the grid refreshes.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";

export interface GroupMember {
  hash: string;
  fileName: string;
  width: number | null;
  height: number | null;
  sharpness: number | null;
  copyCount: number;
  hasThumb: boolean;
}

export const SLOT_KEYS = [
  "1", "2", "3", "4", "5", "6", "7", "8", "9", "0",
  "a", "b", "c", "d", "e", "f",
] as const;

interface ComparisonState {
  open: boolean;
  slots: GroupMember[];
  queue: GroupMember[];
  kept: Set<string>;
  busy: boolean;
  openGroup: (hash: string) => Promise<void>;
  toggleKeep: (slotIndex: number) => void;
  commitTurn: (permanent: boolean) => Promise<void>;
  close: () => void;
}

async function refreshAfterChange(): Promise<void> {
  const { useItemsStore } = await import("./items-store");
  await useItemsStore.getState().refresh();
  const { useSectionsStore } = await import("./sections-store");
  await useSectionsStore.getState().loadCounts();
}

export const useComparisonStore = create<ComparisonState>((set, get) => ({
  open: false,
  slots: [],
  queue: [],
  kept: new Set<string>(),
  busy: false,

  openGroup: async (hash) => {
    try {
      const members = await invoke<GroupMember[]>("get_similar_group", { hash });
      if (members.length < 2) return; // ungrouped items open nothing
      set({
        open: true,
        slots: members.slice(0, SLOT_KEYS.length),
        queue: members.slice(SLOT_KEYS.length),
        kept: new Set<string>(),
      });
    } catch (error) {
      log.error("similar group load failed", toErrorFields(error));
    }
  },

  toggleKeep: (slotIndex) => {
    const { slots, kept } = get();
    const member = slots[slotIndex];
    if (!member) return;
    const next = new Set(kept);
    if (next.has(member.hash)) {
      next.delete(member.hash);
    } else {
      next.add(member.hash);
    }
    set({ kept: next });
  },

  commitTurn: async (permanent) => {
    const { slots, queue, kept, busy } = get();
    if (busy) return;
    set({ busy: true });
    try {
      const keepers = slots.filter((s) => kept.has(s.hash));
      const goners = kept.size > 0 ? slots.filter((s) => !kept.has(s.hash)) : [];

      for (const member of goners) {
        await invoke("delete_item", {
          hash: member.hash,
          pathId: null,
          permanent,
        });
      }

      // Keepers stay pinned; freed slots refill from the queue. A no-keeper
      // commit skips the whole turn (those photos remain in the app).
      const survivors = kept.size > 0 ? keepers : [];
      const room = SLOT_KEYS.length - survivors.length;
      const incoming = queue.slice(0, room);
      const nextQueue = queue.slice(room);
      const nextSlots = [...survivors, ...incoming];

      if (incoming.length === 0) {
        // Nothing new to decide: the group is finished.
        set({ open: false, slots: [], queue: [], kept: new Set(), busy: false });
        await refreshAfterChange();
        return;
      }
      set({
        slots: nextSlots,
        queue: nextQueue,
        kept: new Set(survivors.map((s) => s.hash)),
        busy: false,
      });
      if (goners.length > 0) await refreshAfterChange();
    } catch (error) {
      log.error("comparison commit failed", toErrorFields(error));
      set({ busy: false });
    }
  },

  close: () => {
    set({ open: false, slots: [], queue: [], kept: new Set() });
    void refreshAfterChange();
  },
}));
