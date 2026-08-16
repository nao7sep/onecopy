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
import {
  getCurrentWindow,
  availableMonitors,
  currentMonitor,
  PhysicalPosition,
  PhysicalSize,
} from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { log, toErrorFields } from "../repositories";
import { hasOpenModal } from "../utils/modalStack";
import { monitorKey, orderMonitors, priorityFromState } from "../utils/screens";

export interface GroupMember {
  hash: string;
  fileName: string;
  width: number | null;
  height: number | null;
  byteSize: number | null;
  sharpness: number | null;
  copyCount: number;
  hasThumb: boolean;
}

export const SLOT_KEYS = [
  "1", "2", "3", "4", "5", "6", "7", "8", "9", "0",
  "a", "b", "c", "d", "e", "f",
] as const;

/** The slot a keydown selects, or -1 for "not a slot key".
 *
 * Slot keys are bare single characters, so several collide with app commands:
 * SLOT_KEYS[9] is "0" (Cmd/Ctrl+0 resets zoom) and "a" is a slot (Ctrl+A). A
 * modified key always belongs to the other command — flipping a keeper flag
 * there is silent, because the zoom relayout in the same frame hides the badge
 * change, and the next Enter deletes the photo the user meant to keep.
 *
 * Both key paths route through this one function: the local handler in the
 * comparison view, and keys forwarded from a secondary comparison window. */
export function slotIndexForKey(event: {
  key: string;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}): number {
  if (event.metaKey === true || event.ctrlKey === true || event.altKey === true) {
    return -1;
  }
  return (SLOT_KEYS as readonly string[]).indexOf(event.key.toLowerCase());
}

/** What the secondary windows render: contiguous chunks of the slot list,
 * each entry carrying its GLOBAL slot key so 1–9/0/A–F stay one key space. */
export interface ComparisonBroadcast {
  chunks: { member: GroupMember; slotKey: string; kept: boolean }[][];
  queueCount: number;
  /** The group's dominant image orientation, driving each window's grid. */
  portraitDominant: boolean;
}

interface ComparisonState {
  open: boolean;
  slots: GroupMember[];
  queue: GroupMember[];
  kept: Set<string>;
  busy: boolean;
  /** Permanent commits confirm ONCE per comparison session (a per-turn
   * prompt would destroy the keystroke rhythm the view exists for). */
  permanentArmed: boolean;
  /** A Shift+Enter awaiting that one confirmation. */
  pendingPermanentCommit: boolean;
  confirmPermanentCommit: () => Promise<void>;
  cancelPermanentCommit: () => void;
  /** Secondary comparison windows currently open (monitors beyond the first). */
  spreadCount: number;
  /** Per-screen slot capacities (screen 0 = the main window). The turn size is
   * their sum, capped by the 16 slot keys. */
  capacities: number[];
  /** The group's dominant image orientation (drives the slot grids). */
  portraitDominant: boolean;
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
  const { slots, kept, queue, capacities, portraitDominant } =
    useComparisonStore.getState();
  const payload: ComparisonBroadcast = {
    chunks: chunkSlots(slots, kept, capacities),
    queueCount: queue.length,
    portraitDominant,
  };
  void emit("comparison://state", payload);
}

/** How many COLUMNS a window's slot grid takes, so the cells' shape tracks
 * the photos' shape: portrait photos on a landscape screen stand three
 * abreast; landscape photos take a 2×2; landscape photos on a portrait
 * screen stack. The developer's finding was that a wrapping row of
 * fixed-size tiles left every image small however much screen there was —
 * the grid fills the window and lets the cells be as big as the count
 * allows. Derivation: pick the column count whose cell aspect lands nearest
 * the image aspect. */
export function gridColumns(
  slotCount: number,
  containerAspect: number,
  portraitImages: boolean,
): number {
  if (slotCount <= 1) return 1;
  const imageAspect = portraitImages ? 2 / 3 : 3 / 2;
  const ideal = Math.sqrt((slotCount * containerAspect) / imageAspect);
  return Math.min(slotCount, Math.max(1, Math.round(ideal)));
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

/** The monitors the spread will use and the per-screen capacities they imply,
 * resolved BEFORE any window exists.
 *
 * Splitting this out of the window creation is what makes the handshake sound:
 * a secondary window announces itself the moment it mounts, and the main
 * window can only answer if `open` is already true. Creating the windows first
 * and setting the state afterwards left a gap in which that announcement was
 * answered with silence — the window then waited forever for a broadcast that
 * only fires on a state change. Resolve, set state, THEN create.
 *
 * Best-effort: a machine with one monitor keeps the single-window form and all
 * 16 keys. */
async function resolveSpread(
  perScreen: number,
): Promise<{ others: Awaited<ReturnType<typeof availableMonitors>>; capacities: number[] }> {
  try {
    // `others` is every monitor EXCEPT the one hosting the main window —
    // found by ASKING, never assumed. The spread used to target priority
    // slots 2+ blind, so whenever the priority list disagreed with where the
    // main window really was (a moved window; the broken matched-pair keys),
    // an always-on-top borderless window landed ON TOP of the main window
    // and buried its slots — the developer never saw keys 1–4. The main
    // window's own screen hosts chunk 0 by construction now; priority still
    // orders which of the OTHER monitors join first.
    const { useAppStore } = await import("./app-store");
    const monitors = orderMonitors(
      await availableMonitors(),
      priorityFromState(useAppStore.getState().appData?.state ?? null),
    );
    const hosting = await currentMonitor().catch(() => null);
    const hostKey = hosting !== null ? monitorKey(hosting) : null;
    const others =
      hostKey === null
        ? monitors.slice(1)
        : monitors.filter((m) => monitorKey(m) !== hostKey);
    return {
      others,
      capacities:
        others.length === 0
          ? [SLOT_KEYS.length]
          : [perScreen, ...others.map(() => perScreen)],
    };
  } catch (error) {
    log.warn("monitor query failed; staying single-window", toErrorFields(error));
    return { others: [], capacities: [SLOT_KEYS.length] };
  }
}

/** Creates (or reveals) one borderless window per extra monitor, sized to that
 * monitor's own bounds.
 *
 * Deliberately NOT the OS fullscreen call: on macOS that animates the window
 * into its own Space, which costs about a second every time a group opens and
 * again when it closes — unusable in a keystroke-paced culling flow. A
 * frameless window placed at the monitor's exact bounds and held above the
 * others looks the same and appears instantly (imagequeue's viewer proves the
 * approach). */
async function openSpread(
  others: Awaited<ReturnType<typeof availableMonitors>>,
): Promise<void> {
  try {
    for (let i = 0; i < others.length; i += 1) {
      const label = `comparison-${i + 1}`;
      const existing = await WebviewWindow.getByLabel(label);
      const monitor = others[i];
      if (existing !== null) {
        // Reused from a previous session: reveal and re-place it, cheaper
        // than a webview boot and it keeps its listener registered.
        // A monitor reports PHYSICAL pixels, so place it with the physical
        // types rather than converting — on a Retina display the logical
        // numbers are half these, and a half-sized window would be the bug.
        await existing.setPosition(
          new PhysicalPosition(monitor.position.x, monitor.position.y),
        );
        await existing.setSize(new PhysicalSize(monitor.size.width, monitor.size.height));
        await existing.show();
        await existing.setFocus();
        continue;
      }
      // The constructor's x/y/width/height are LOGICAL, so the monitor's
      // physical bounds are divided by its own scale factor here.
      const scale = monitor.scaleFactor || 1;
      new WebviewWindow(label, {
        url: `index.html?view=comparison&slice=${i + 1}`,
        title: "OneCopy Comparison",
        x: monitor.position.x / scale,
        y: monitor.position.y / scale,
        width: monitor.size.width / scale,
        height: monitor.size.height / scale,
        decorations: false,
        alwaysOnTop: true,
        skipTaskbar: true,
        resizable: false,
      });
    }
  } catch (error) {
    log.warn("comparison spread failed; staying single-window", toErrorFields(error));
  }
}

/** HIDES the spread rather than closing it. A hidden window keeps its webview
 * and its `comparison://state` listener, so the next group opens without a
 * boot — the same reuse imagequeue's viewer relies on. They are real windows
 * owned by the app and go away with it. */
async function closeSpread(): Promise<void> {
  const { spreadCount } = useComparisonStore.getState();
  for (let i = 1; i <= spreadCount; i += 1) {
    const window = await WebviewWindow.getByLabel(`comparison-${i}`).catch(() => null);
    if (window !== null) await window.hide().catch(() => {});
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
  permanentArmed: false,
  pendingPermanentCommit: false,
  spreadCount: 0,
  capacities: [SLOT_KEYS.length],
  portraitDominant: false,

  confirmPermanentCommit: async () => {
    set({ permanentArmed: true, pendingPermanentCommit: false });
    await get().commitTurn(true);
  },

  cancelPermanentCommit: () => set({ pendingPermanentCommit: false }),

  openGroup: async (hash) => {
    try {
      const members = await invoke<GroupMember[]>("get_similar_group", { hash });
      if (members.length < 2) {
        // A ≈ badge whose group lost its other members (deleted, drive
        // absent) must not swallow Enter silently.
        log.warn("similar group has fewer than 2 live members", { hash });
        return false;
      }
      // Resolve the screens first (their capacities decide the turn size),
      // publish the state, and only THEN create the windows — a window that
      // announces itself must find a session already open to be answered.
      const perScreen = perScreenCapacity(members);
      const { others, capacities } = await resolveSpread(perScreen);
      const size = turnSize(capacities);
      set({
        open: true,
        capacities,
        spreadCount: others.length,
        portraitDominant: perScreen === 3,
        slots: members.slice(0, size),
        queue: members.slice(size),
        kept: new Set<string>(),
        // A new comparison session re-arms the one permanent confirmation.
        permanentArmed: false,
        pendingPermanentCommit: false,
      });
      broadcast();
      await openSpread(others);
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
    const { slots, queue, kept, busy, permanentArmed } = get();
    if (busy) return;
    if (permanent && !permanentArmed) {
      set({ pendingPermanentCommit: true });
      return;
    }
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
      const size = turnSize(get().capacities);
      const room = size - survivors.length;
      // Keeping every slot leaves no room to refill — but the queue's members
      // have not been seen yet, and dropping them would hide part of the group
      // permanently (reopening refills with the same keepers). Advance to a
      // fresh turn from the queue instead, exactly as a no-keeper commit does.
      // The keepers are already decided: kept means not deleted.
      const pinned = room > 0 ? survivors : [];
      const intake = room > 0 ? room : size;
      const incoming = queue.slice(0, intake);
      const nextQueue = queue.slice(intake);
      const nextSlots = [...pinned, ...incoming];

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
        kept: new Set(pinned.map((s) => s.hash)),
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
    set({
      open: false,
      slots: [],
      queue: [],
      kept: new Set(),
      permanentArmed: false,
      pendingPermanentCommit: false,
    });
    void closeSpread();
    void refreshAfterChange();
  },
}));

// Main-window-only wiring: secondary comparison windows forward their keys and
// ask for the current state on load; the main window owns all mutations.
void (async () => {
  try {
    if (getCurrentWindow().label !== "main") return;
    await listen<{
      key: string;
      shiftKey: boolean;
      metaKey?: boolean;
      ctrlKey?: boolean;
      altKey?: boolean;
    }>("comparison://key", (event) => {
      const store = useComparisonStore.getState();
      if (!store.open) return;
      // A modal open in the main window owns the keyboard for forwarded
      // keys too — a secondary screen's Escape must not tear the session
      // down from under an open dialog.
      if (hasOpenModal()) return;
      const slotIndex = slotIndexForKey(event.payload);
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
