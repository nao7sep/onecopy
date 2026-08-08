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
  openGroup: (hash: string) => Promise<void>;
  toggleKeep: (slotIndex: number) => void;
  commitTurn: (permanent: boolean) => Promise<void>;
  close: () => void;
}

/** Chunks the slots across 1 + spreadCount screens, contiguous and even. */
export function chunkSlots(
  slots: GroupMember[],
  kept: Set<string>,
  screens: number,
): ComparisonBroadcast["chunks"] {
  const total = Math.max(1, screens);
  const per = Math.ceil(slots.length / total);
  const chunks: ComparisonBroadcast["chunks"] = [];
  for (let s = 0; s < total; s += 1) {
    chunks.push(
      slots.slice(s * per, (s + 1) * per).map((member, i) => ({
        member,
        slotKey: SLOT_KEYS[s * per + i] ?? "?",
        kept: kept.has(member.hash),
      })),
    );
  }
  return chunks;
}

function broadcast(): void {
  const { slots, kept, queue, spreadCount } = useComparisonStore.getState();
  const payload: ComparisonBroadcast = {
    chunks: chunkSlots(slots, kept, 1 + spreadCount),
    queueCount: queue.length,
  };
  void emit("comparison://state", payload);
}

// Spreads the comparison across every extra monitor: one fullscreen window
// per monitor beyond the first. Best-effort — a machine with one monitor
// simply keeps the single-window form.
async function openSpread(): Promise<void> {
  try {
    const monitors = await availableMonitors();
    const extras = Math.max(0, monitors.length - 1);
    useComparisonStore.setState({ spreadCount: extras });
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
    useComparisonStore.setState({ spreadCount: 0 });
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
      await openSpread();
      broadcast();
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
      const room = SLOT_KEYS.length - survivors.length;
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
