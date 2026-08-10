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
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow, availableMonitors } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { log, toErrorFields } from "../repositories";
import { hasOpenModal } from "../utils/modalStack";

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

/** What the secondary windows render: contiguous chunks of the slot list,
 * each entry carrying its GLOBAL slot key so 1–9/0/A–F stay one key space. */
export interface ComparisonBroadcast {
  chunks: { member: GroupMember; slotKey: string; kept: boolean }[][];
  queueCount: number;
}

interface ComparisonState {
  open: boolean;
  slots: GroupMember[];
  queue: GroupMember[];
  kept: Set<string>;
  busy: boolean;
  /** Secondary comparison windows currently open (monitors beyond the first). */
  spreadCount: number;
  /** Per-screen slot capacities (screen 0 = the main window). The turn size is
   * their sum, capped by the 16 slot keys. */
  capacities: number[];
  /** Resolves false when no live group opened (fewer than 2 members) so the
   * caller can fall back honestly instead of doing nothing on Enter. */
  openGroup: (hash: string) => Promise<boolean>;
  toggleKeep: (slotIndex: number) => void;
  commitTurn: (permanent: boolean) => Promise<void>;
  close: () => void;
}

/** Chunks the slots across screens by their capacities, contiguous, keeping
 * ONE global key space (the design's 3-vertical / 4-horizontal per screen). */
export function chunkSlots(
  slots: GroupMember[],
  kept: Set<string>,
  capacities: number[],
): ComparisonBroadcast["chunks"] {
  const caps = capacities.length > 0 ? capacities : [slots.length];
  const chunks: ComparisonBroadcast["chunks"] = [];
  let offset = 0;
  for (const capacity of caps) {
    chunks.push(
      slots.slice(offset, offset + capacity).map((member, i) => ({
        member,
        slotKey: SLOT_KEYS[offset + i] ?? "?",
        kept: kept.has(member.hash),
      })),
    );
    offset += capacity;
  }
  return chunks;
}

export function turnSize(capacities: number[]): number {
  const sum = capacities.reduce((a, b) => a + b, 0);
  return Math.min(SLOT_KEYS.length, Math.max(1, sum));
}

function broadcast(): void {
  const { slots, kept, queue, capacities } = useComparisonStore.getState();
  const payload: ComparisonBroadcast = {
    chunks: chunkSlots(slots, kept, capacities),
    queueCount: queue.length,
  };
  void emit("comparison://state", payload);
}

/// The design's per-screen rule: three slots when the photos run portrait,
/// four when they run landscape — decided by the GROUP's dominant image
/// orientation (unknown dimensions count as landscape).
export function perScreenCapacity(members: GroupMember[]): number {
  const portrait = members.filter(
    (m) => m.width !== null && m.height !== null && m.height > m.width,
  ).length;
  return portrait * 2 > members.length ? 3 : 4;
}

// Spreads the comparison across every extra monitor: one fullscreen window
// per monitor beyond the first, per-screen capacity from the group's dominant
// image orientation. Best-effort — a machine with one monitor keeps the
// single-window form and all 16 keys.
async function openSpread(perScreen: number): Promise<void> {
  try {
    const monitors = await availableMonitors();
    const extras = Math.max(0, monitors.length - 1);
    const capacities =
      extras === 0 ? [SLOT_KEYS.length] : monitors.map(() => perScreen);
    useComparisonStore.setState({ spreadCount: extras, capacities });
    for (let i = 0; i < extras; i += 1) {
      const label = `comparison-${i + 1}`;
      const existing = await WebviewWindow.getByLabel(label);
      if (existing !== null) continue;
      const window = new WebviewWindow(label, {
        url: `index.html?view=comparison&slice=${i + 1}`,
        title: "OneCopy Comparison",
        x: monitors[i + 1].position.x,
        y: monitors[i + 1].position.y,
        width: 1024,
        height: 768,
      });
      void window.once("tauri://created", () => {
        void window.setFullscreen(true).catch(() => {});
      });
    }
  } catch (error) {
    log.warn("comparison spread failed; staying single-window", toErrorFields(error));
    useComparisonStore.setState({ spreadCount: 0, capacities: [SLOT_KEYS.length] });
  }
}

async function closeSpread(): Promise<void> {
  const { spreadCount } = useComparisonStore.getState();
  for (let i = 1; i <= spreadCount; i += 1) {
    const window = await WebviewWindow.getByLabel(`comparison-${i}`).catch(() => null);
    if (window !== null) await window.close().catch(() => {});
  }
  useComparisonStore.setState({ spreadCount: 0 });
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
  spreadCount: 0,
  capacities: [SLOT_KEYS.length],

  openGroup: async (hash) => {
    try {
      const members = await invoke<GroupMember[]>("get_similar_group", { hash });
      if (members.length < 2) {
        // A ≈ badge whose group lost its other members (deleted, drive
        // absent) must not swallow Enter silently.
        log.warn("similar group has fewer than 2 live members", { hash });
        return false;
      }
      // Spread first: the screens' capacities decide the turn size.
      await openSpread(perScreenCapacity(members));
      const size = turnSize(get().capacities);
      set({
        open: true,
        slots: members.slice(0, size),
        queue: members.slice(size),
        kept: new Set<string>(),
      });
      broadcast();
      return true;
    } catch (error) {
      log.error("similar group load failed", toErrorFields(error));
      return false;
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
    broadcast();
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
      const room = turnSize(get().capacities) - survivors.length;
      const incoming = queue.slice(0, room);
      const nextQueue = queue.slice(room);
      const nextSlots = [...survivors, ...incoming];

      if (incoming.length === 0) {
        // Nothing new to decide: the group is finished.
        set({ open: false, slots: [], queue: [], kept: new Set(), busy: false });
        void closeSpread();
        await refreshAfterChange();
        return;
      }
      set({
        slots: nextSlots,
        queue: nextQueue,
        kept: new Set(survivors.map((s) => s.hash)),
        busy: false,
      });
      broadcast();
      if (goners.length > 0) await refreshAfterChange();
    } catch (error) {
      log.error("comparison commit failed", toErrorFields(error));
      set({ busy: false });
    }
  },

  close: () => {
    set({ open: false, slots: [], queue: [], kept: new Set() });
    void closeSpread();
    void refreshAfterChange();
  },
}));

// Main-window-only wiring: secondary comparison windows forward their keys and
// ask for the current state on load; the main window owns all mutations.
void (async () => {
  try {
    if (getCurrentWindow().label !== "main") return;
    await listen<{ key: string; shiftKey: boolean }>("comparison://key", (event) => {
      const store = useComparisonStore.getState();
      if (!store.open) return;
      // A modal open in the main window owns the keyboard for forwarded
      // keys too — a secondary screen's Escape must not tear the session
      // down from under an open dialog.
      if (hasOpenModal()) return;
      const key = event.payload.key.toLowerCase();
      const slotIndex = (SLOT_KEYS as readonly string[]).indexOf(key);
      if (slotIndex >= 0) {
        store.toggleKeep(slotIndex);
      } else if (event.payload.key === "Enter") {
        void store.commitTurn(event.payload.shiftKey);
      } else if (event.payload.key === "Escape") {
        store.close();
      }
    });
    await listen("comparison://ready", () => {
      if (useComparisonStore.getState().open) broadcast();
    });
  } catch (error) {
    log.warn("comparison spread wiring failed", toErrorFields(error));
  }
})();
