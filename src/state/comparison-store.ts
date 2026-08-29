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
import { requestSeq } from "./request-seq";
import { emit } from "@tauri-apps/api/event";
import {
  getCurrentWindow,
  availableMonitors,
  currentMonitor,
  PhysicalPosition,
  PhysicalSize,
} from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { log, toErrorFields, reportWindowCall } from "../repositories";
import { monitorKey, orderMonitors, priorityFromState } from "../utils/screens";

export interface GroupMember {
  hash: string;
  fileName: string;
  width: number | null;
  height: number | null;
  byteSize: number | null;
  sharpness: number | null;
  faceScore: number | null;
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

/** The slot a SHIFTED keydown unlinks, or -1. Matched on `event.code` — the
 * layout-independent physical key — because Shift+1 delivers `key: "!"` on
 * US layouts and other symbols elsewhere; the physical digit row is the one
 * thing every layout shares. Bare `event.key` matching (the keeper path)
 * cannot express this, which is why the two resolvers stay separate. */
export function slotIndexForShiftedCode(event: {
  code?: string;
  shiftKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}): number {
  if (event.shiftKey !== true) return -1;
  if (event.metaKey === true || event.ctrlKey === true || event.altKey === true) {
    return -1;
  }
  const code = event.code ?? "";
  if (code.startsWith("Digit")) {
    const digit = code.slice(5);
    if (digit === "0") return 9;
    const n = Number.parseInt(digit, 10);
    return n >= 1 && n <= 9 ? n - 1 : -1;
  }
  if (code.startsWith("Key")) {
    const index = (SLOT_KEYS as readonly string[]).indexOf(code.slice(3).toLowerCase());
    return index >= 10 ? index : -1;
  }
  return -1;
}

/** What the secondary windows render: contiguous chunks of the slot list,
 * each entry carrying its GLOBAL slot key so 1–9/0/A–F stay one key space. */
export interface ComparisonBroadcast {
  /** `member: null` is an unlinked slot — rendered as an empty cell so the
   * other slots keep their key numbers for the rest of the turn. */
  chunks: { member: GroupMember | null; slotKey: string; kept: boolean }[][];
  queueCount: number;
  /** The group's dominant image orientation, driving each window's grid. */
  portraitDominant: boolean;
}

interface ComparisonState {
  open: boolean;
  /** EVERY member of the group, in session order (best-first). null = a slot
   * unlinked this session — the hole keeps page geometry and key numbers
   * stable. Pages are a VIEWPORT over this list; nothing else is state. */
  members: (GroupMember | null)[];
  /** Keep marks, by hash — they persist across pages (the paged model,
   * Phase 33: viewing and deciding are decoupled; navigation never deletes). */
  kept: Set<string>;
  /** Pages the user has SEEN. The safety rule of the whole design: nothing
   * from an unvisited page can ever be deleted, guaranteed by Enter
   * advancing through unseen pages before it will commit. */
  visited: Set<number>;
  page: number;
  /** The shortlist view (S): the current marks as their own paged viewport —
   * the passport-photo finale, candidates compared side by side at the END. */
  shortlist: boolean;
  shortlistPage: number;
  busy: boolean;
  /** A partially completed commit stays in-session so Retry can target only
   * the logical items whose files remain. */
  commitFailure: { message: string; permanent: boolean } | null;
  /** Permanent commits confirm ONCE per comparison session (a per-turn
   * prompt would destroy the keystroke rhythm the view exists for). */
  permanentArmed: boolean;
  /** A Shift+Enter awaiting that one confirmation. */
  pendingPermanentCommit: boolean;
  /** A commit awaiting its count confirmation — zero marks (trash ALL), a
   * multi-page group, or the confirmTrashDelete config. Single-page commits
   * with at least one mark stay two keystrokes with no dialog. */
  pendingCommit: { keepCount: number; trashCount: number; permanent: boolean } | null;
  confirmPendingCommit: () => Promise<ComparisonCommitResult | null>;
  cancelPendingCommit: () => void;
  confirmPermanentCommit: (
    configConfirms?: boolean,
  ) => Promise<ComparisonCommitResult | null>;
  cancelPermanentCommit: () => void;
  /** Secondary comparison windows currently open (monitors beyond the first). */
  spreadCount: number;
  /** Per-screen slot capacities (screen 0 = the main window). The page size
   * is their sum, capped by the 16 slot keys. */
  capacities: number[];
  /** The group's dominant image orientation (drives the slot grids). */
  portraitDominant: boolean;
  /** Every original member hash still in the family — what the finish
   * advances past. Unlinking removes the image here too. */
  sessionMembers: string[];
  openGroup: (
    hash: string,
    screenState?: Record<string, unknown>,
  ) => Promise<boolean>;
  /** Toggles the keep mark of a slot ON THE VISIBLE PAGE. */
  toggleKeep: (slotIndex: number) => void;
  /** The unlink: this visible slot's image is NOT the same subject.
   * Persistent core-side; the slot becomes a hole. */
  unlinkSlot: (slotIndex: number) => Promise<void>;
  nextPage: () => void;
  prevPage: () => void;
  toggleShortlist: () => void;
  /** Enter: advance to the next unseen page, or — once every page has been
   * seen — commit the whole group (keep the marked, trash the rest),
   * confirming by the policy above. */
  commitTurn: (
    permanent: boolean,
    configConfirms?: boolean,
  ) => Promise<ComparisonCommitResult | null>;
  close: () => Promise<void>;
}

export type ComparisonCommitResult =
  | { kind: "failed" }
  | { kind: "completed"; family: string[] };

/** The page geometry: fixed windows of `pageSize` over the member list. */
export function pageCountOf(memberCount: number, pageSize: number): number {
  return Math.max(1, Math.ceil(memberCount / Math.max(1, pageSize)));
}

/** What the screens show right now: the current page, or the current
 * shortlist page (marks only, live members, re-sliced by the same size). */
export function visibleSlots(state: {
  members: (GroupMember | null)[];
  kept: Set<string>;
  page: number;
  shortlist: boolean;
  shortlistPage: number;
  capacities: number[];
}): (GroupMember | null)[] {
  const size = turnSize(state.capacities);
  if (state.shortlist) {
    const marked = state.members.filter(
      (m): m is GroupMember => m !== null && state.kept.has(m.hash),
    );
    return marked.slice(state.shortlistPage * size, (state.shortlistPage + 1) * size);
  }
  return state.members.slice(state.page * size, (state.page + 1) * size);
}

/** How many photos the user is actually looking at. NOT `slots.length`: an
 * unlinked slot stays in the array as a hole so the keys after it keep their
 * numbers, and counting holes made the header claim photos that are not on
 * screen ("2 kept · 4 shown" over three photos and a gap). */
export function liveSlotCount(slots: (GroupMember | null)[]): number {
  return slots.reduce((n, slot) => (slot === null ? n : n + 1), 0);
}

/** Chunks the slots across screens by their capacities, contiguous, keeping
 * ONE global key space (the design's 3-vertical / 4-horizontal per screen). */
export function chunkSlots(
  slots: (GroupMember | null)[],
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
        kept: member !== null && kept.has(member.hash),
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

export function broadcastComparison(): void {
  const state = useComparisonStore.getState();
  const visible = visibleSlots(state);
  const size = turnSize(state.capacities);
  const payload: ComparisonBroadcast = {
    chunks: chunkSlots(visible, state.kept, state.capacities),
    // For the windows' footer: photos living on OTHER pages of this view.
    queueCount: Math.max(
      0,
      (state.shortlist
        ? state.members.filter((m) => m !== null && state.kept.has(m.hash)).length
        : state.members.length) - visible.length - (state.shortlist ? state.shortlistPage : state.page) * size,
    ),
    portraitDominant: state.portraitDominant,
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
/** How many screens this family actually fills. A 6-member family on three
 * 4-slot screens is 4 + 2 + NOTHING — and a spread window with nothing to
 * show must not open at all: an empty always-on-top surface covering a whole
 * monitor is not a comparison aid, it is a curtain. Never below 1 (the main
 * window always hosts chunk 0), never above what exists. */
export function screensNeeded(
  memberCount: number,
  perScreen: number,
  available: number,
): number {
  return Math.min(available, Math.max(1, Math.ceil(memberCount / perScreen)));
}

async function resolveSpread(
  perScreen: number,
  memberCount: number,
  screenState: Record<string, unknown>,
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
    const monitors = orderMonitors(
      await availableMonitors(),
      priorityFromState(screenState),
    );
    const hosting = await currentMonitor();
    const hostKey = hosting !== null ? monitorKey(hosting) : null;
    const eligible =
      hostKey === null
        ? monitors.slice(1)
        : monitors.filter((m) => monitorKey(m) !== hostKey);
    // Only as many screens as the family fills (screensNeeded's contract);
    // priority order decides WHICH of the eligible monitors join.
    const others = eligible.slice(
      0,
      screensNeeded(memberCount, perScreen, eligible.length + 1) - 1,
    );
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
): Promise<boolean> {
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
        // macOS: cover the menu bar and dock too (simple fullscreen — no
        // Spaces animation). No-op elsewhere; borderless-at-bounds already
        // covers the Windows taskbar.
        await setComparisonFullscreen(label, true).catch(
          reportWindowCall("comparison enter fullscreen"),
        );
        continue;
      }
      // The constructor's x/y/width/height are LOGICAL, so the monitor's
      // physical bounds are divided by its own scale factor here.
      const scale = monitor.scaleFactor || 1;
      const created = new WebviewWindow(label, {
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
        focus: false,
      });
      void created.once("tauri://created", () => {
        const state = useComparisonStore.getState();
        if (!state.open || i >= state.spreadCount) return;
        void setComparisonFullscreen(label, true)
          .catch(reportWindowCall("comparison enter fullscreen"))
          .then(() =>
            getCurrentWindow().setFocus().catch(reportWindowCall("main setFocus")),
          );
      });
      void created.once("tauri://error", (event) => {
        log.warn("comparison window creation failed", {
          label,
          error: { message: String(event.payload) },
        });
        recoverSingleWindowComparison();
      });
    }
    return true;
  } catch (error) {
    log.warn("comparison spread failed; staying single-window", toErrorFields(error));
    return false;
  }
}

/** A valid comparison claims the screens its spread will use. Hide an
 * existing Preview window only after the session is published, immediately
 * before spread creation; an invalid/stale group must never flicker Preview. */
async function hidePreviewWindowForComparison(): Promise<void> {
  const preview = await WebviewWindow.getByLabel("preview").catch((error) => {
    reportWindowCall("preview lookup")(error);
    return null;
  });
  if (preview !== null) {
    await preview.hide().catch(reportWindowCall("preview hide"));
  }
}

function recoverSingleWindowComparison(): void {
  const state = useComparisonStore.getState();
  if (!state.open || state.spreadCount === 0) return;
  const spreadCount = state.spreadCount;
  useComparisonStore.setState({ capacities: [SLOT_KEYS.length], spreadCount: 0 });
  broadcastComparison();
  void queueComparisonLifecycle(async () => {
    await closeSpread(spreadCount);
    await getCurrentWindow().setFocus().catch(reportWindowCall("main setFocus"));
  });
}

/** HIDES the spread rather than closing it. A hidden window keeps its webview
 * and its `comparison://state` listener, so the next group opens without a
 * boot — the same reuse imagequeue's viewer relies on. They are real windows
 * owned by the app and go away with it. */
async function closeSpread(spreadCount: number): Promise<void> {
  for (let i = 1; i <= spreadCount; i += 1) {
    const label = `comparison-${i}`;
    const window = await WebviewWindow.getByLabel(label).catch((error) => {
      reportWindowCall("comparison window lookup")(error);
      return null;
    });
    if (window !== null) {
      // Leave simple fullscreen BEFORE hiding: a hidden simple-fullscreen
      // window reappears in a broken half-state on macOS.
      await setComparisonFullscreen(label, false).catch(
        reportWindowCall("comparison leave fullscreen"),
      );
      await window.hide().catch(reportWindowCall("comparison hide"));
    }
  }
}

const fullscreenTransitions = new Map<string, Promise<void>>();

/** A label has one ordered native fullscreen transition stream. */
function setComparisonFullscreen(label: string, enable: boolean): Promise<void> {
  const previous = fullscreenTransitions.get(label) ?? Promise.resolve();
  const next = previous
    .catch(() => undefined)
    .then(() => invoke<void>("set_window_simple_fullscreen", { label, enable }));
  fullscreenTransitions.set(label, next);
  const clear = () => {
    if (fullscreenTransitions.get(label) === next) fullscreenTransitions.delete(label);
  };
  void next.then(clear, clear);
  return next;
}

let comparisonLifecycle: Promise<void> = Promise.resolve();

/** Opening and closing claimed screens are one serialized lifecycle. */
function queueComparisonLifecycle(action: () => Promise<void>): Promise<void> {
  const next = comparisonLifecycle.then(action, action);
  comparisonLifecycle = next.catch(() => undefined);
  return next;
}

async function teardownComparison(spreadCount: number): Promise<void> {
  await closeSpread(spreadCount);
  await getCurrentWindow().setFocus().catch(reportWindowCall("main setFocus"));
}

const groupLoad = requestSeq();

export const useComparisonStore = create<ComparisonState>((set, get) => ({
  open: false,
  members: [],
  kept: new Set<string>(),
  visited: new Set<number>(),
  page: 0,
  shortlist: false,
  shortlistPage: 0,
  sessionMembers: [],
  busy: false,
  commitFailure: null,
  permanentArmed: false,
  pendingPermanentCommit: false,
  pendingCommit: null,
  spreadCount: 0,
  capacities: [SLOT_KEYS.length],
  portraitDominant: false,

  confirmPermanentCommit: async (configConfirms = false) => {
    set({ permanentArmed: true, pendingPermanentCommit: false });
    return await get().commitTurn(true, configConfirms);
  },

  cancelPermanentCommit: () => set({ pendingPermanentCommit: false }),

  confirmPendingCommit: async () => {
    const pending = get().pendingCommit;
    if (pending === null) return null;
    set({ pendingCommit: null });
    return await doCommit(set, get, pending.permanent);
  },

  cancelPendingCommit: () => set({ pendingCommit: null }),

  openGroup: async (hash, screenState = {}) => {
    // get_similar_group is an async command: two quick Enters on different
    // anchors race, and the OLDER group's continuation must not publish state
    // or open windows over the newer one's (request-seq.ts).
    const fresh = groupLoad.begin();
    try {
      const members = await invoke<GroupMember[]>("get_similar_group", { hash });
      // True, not false: the newer call owns the outcome now, and false would
      // send the caller down its "group vanished" path for a group that is
      // simply someone else's.
      if (!fresh()) return true;
      if (members.length < 2) {
        // A ≈ badge whose group lost its other members (deleted, drive
        // absent) must not swallow Enter silently.
        log.warn("similar group has fewer than 2 live members", { hash });
        return false;
      }
      // Resolve the screens first (their capacities decide the page size),
      // publish the state, and only THEN create the windows — a window that
      // announces itself must find a session already open to be answered.
      const perScreen = perScreenCapacity(members);
      const { others, capacities } = await resolveSpread(
        perScreen,
        members.length,
        screenState,
      );
      await queueComparisonLifecycle(async () => {
        if (!fresh()) return;
        set({
          open: true,
          capacities,
          spreadCount: others.length,
          portraitDominant: perScreen === 3,
          members,
          kept: new Set<string>(),
          visited: new Set([0]),
          page: 0,
          shortlist: false,
          shortlistPage: 0,
          sessionMembers: members.map((m) => m.hash),
          // A new comparison session re-arms the one permanent confirmation.
          permanentArmed: false,
          pendingPermanentCommit: false,
          pendingCommit: null,
          commitFailure: null,
        });
        broadcastComparison();
        await hidePreviewWindowForComparison();
        const spreadOpened = await openSpread(others);
        if (!spreadOpened) {
          set({ capacities: [SLOT_KEYS.length], spreadCount: 0 });
          broadcastComparison();
          await closeSpread(others.length);
        }
        await getCurrentWindow().setFocus().catch(reportWindowCall("main setFocus"));
      });
      return true;
    } catch (error) {
      log.error("similar group load failed", toErrorFields(error));
      return false;
    }
  },

  toggleKeep: (slotIndex) => {
    if (get().busy) return;
    const member = visibleSlots(get())[slotIndex];
    if (!member) return;
    const next = new Set(get().kept);
    if (next.has(member.hash)) {
      next.delete(member.hash);
    } else {
      next.add(member.hash);
    }
    set({ kept: next });
    broadcastComparison();
  },

  unlinkSlot: async (slotIndex) => {
    const { busy, sessionMembers } = get();
    if (busy) return;
    const member = visibleSlots(get())[slotIndex];
    if (!member) return;
    try {
      await invoke("similar_unlink", { hash: member.hash });
    } catch (error) {
      log.error("similar unlink failed", { hash: member.hash, ...toErrorFields(error) });
      return;
    }
    const nextKept = new Set(get().kept);
    nextKept.delete(member.hash);
    set({
      // The hole stays: page geometry and key numbers hold for the session.
      members: get().members.map((m) => (m !== null && m.hash === member.hash ? null : m)),
      kept: nextKept,
      // No longer family: the finish may land the anchor on it, which is the
      // natural way to meet the intruder again and decide its own fate.
      sessionMembers: sessionMembers.filter((h) => h !== member.hash),
    });
    broadcastComparison();
  },

  nextPage: () => {
    if (!get().busy) movePage(set, get, 1);
  },
  prevPage: () => {
    if (!get().busy) movePage(set, get, -1);
  },

  toggleShortlist: () => {
    if (get().busy) return;
    set({ shortlist: !get().shortlist, shortlistPage: 0 });
    broadcastComparison();
  },

  commitTurn: async (permanent, configConfirms = false) => {
    const state = get();
    if (state.busy) return null;
    const retrying = state.commitFailure !== null;
    const commitPermanent = state.commitFailure?.permanent ?? permanent;
    const size = turnSize(state.capacities);
    const pages = pageCountOf(state.members.length, size);
    // The advance half of Enter's rhythm: while unseen pages remain, Enter
    // means "next unseen page" — look, mark, Enter, unchanged muscle memory —
    // and it is what GUARANTEES nothing unseen can be deleted.
    if (!state.shortlist) {
      const unseen = nextUnseenPage(state.visited, state.page, pages);
      if (unseen !== null) {
        set({
          page: unseen,
          visited: new Set(state.visited).add(unseen),
        });
        broadcastComparison();
        return null;
      }
    } else if (state.visited.size < pages) {
      // Committing FROM the shortlist still requires every page seen.
      set({ shortlist: false });
      return await get().commitTurn(permanent, configConfirms);
    }
    if (commitPermanent && !state.permanentArmed) {
      set({ pendingPermanentCommit: true });
      return null;
    }
    const live = state.members.filter((m): m is GroupMember => m !== null);
    const trashCount = live.filter((m) => !state.kept.has(m.hash)).length;
    // The confirmation policy, tuned for pace: a single-page group with at
    // least one mark commits instantly (1, Enter — two keystrokes); the
    // count dialog appears for zero marks (trash ALL — the gesture that was
    // impossible before), for multi-page trashing, and under the opt-in
    // confirmTrashDelete config. Nothing-to-trash commits are free.
    const needsConfirm =
      !retrying &&
      trashCount > 0 &&
      (state.kept.size === 0 || pages > 1 || configConfirms);
    if (needsConfirm) {
      set({
        pendingCommit: {
          keepCount: state.kept.size,
          trashCount,
          permanent: commitPermanent,
        },
      });
      return null;
    }
    return await doCommit(set, get, commitPermanent);
  },

  close: async () => {
    if (!get().open || get().busy) return;
    const spreadCount = get().spreadCount;
    set(closedComparisonState());
    await queueComparisonLifecycle(() => teardownComparison(spreadCount));
  },
}));

/** The next unseen page at or after `from`, wrapping — or null when all seen. */
export function nextUnseenPage(
  visited: Set<number>,
  from: number,
  pageCount: number,
): number | null {
  for (let step = 1; step <= pageCount; step += 1) {
    const candidate = (from + step) % pageCount;
    if (!visited.has(candidate)) return candidate;
  }
  return null;
}

function movePage(
  set: (partial: Partial<ComparisonState>) => void,
  get: () => ComparisonState,
  delta: number,
): void {
  const state = get();
  const size = turnSize(state.capacities);
  if (state.shortlist) {
    const marked = state.members.filter(
      (m) => m !== null && state.kept.has(m.hash),
    ).length;
    const pages = pageCountOf(marked, size);
    const next = Math.min(pages - 1, Math.max(0, state.shortlistPage + delta));
    if (next !== state.shortlistPage) {
      set({ shortlistPage: next });
      broadcastComparison();
    }
    return;
  }
  const pages = pageCountOf(state.members.length, size);
  const next = Math.min(pages - 1, Math.max(0, state.page + delta));
  if (next !== state.page) {
    set({ page: next, visited: new Set(state.visited).add(next) });
    broadcastComparison();
  }
}

/** The commit itself: keep the marked, trash the rest, close, chain. Only
 * reachable once every page has been seen and any confirmation has passed. */
async function doCommit(
  set: (partial: Partial<ComparisonState>) => void,
  get: () => ComparisonState,
  permanent: boolean,
): Promise<ComparisonCommitResult> {
  const state = get();
  set({ busy: true, commitFailure: null });
  const live = state.members.filter((m): m is GroupMember => m !== null);
  const goners = live.filter((m) => !state.kept.has(m.hash));
  let outcome: {
    cancelled: boolean;
    error: string | null;
    failedFiles: number;
    items: Array<{ item: { hash: string | null; pathId: number | null }; failedFiles: number }>;
  };
  try {
    outcome = await invoke<typeof outcome>("delete_items", {
      items: goners.map((member) => ({ hash: member.hash, pathId: null })),
      permanent,
    });
  } catch (error) {
    log.error("comparison commit failed", toErrorFields(error));
    set({
      busy: false,
      commitFailure: {
        message: "The delete operation could not finish. Retry targets the still-indexed items.",
        permanent,
      },
    });
    return { kind: "failed" };
  }
  const completed = new Set(
    outcome.items
      .filter((result) => result.failedFiles === 0 && result.item.hash !== null)
      .map((result) => result.item.hash as string),
  );
  if (completed.size > 0) {
    set({
      members: get().members.map((candidate) =>
        candidate !== null && completed.has(candidate.hash) ? null : candidate,
      ),
    });
    broadcastComparison();
  }
  const remainingItems = goners.length - completed.size;
  if (remainingItems > 0 || outcome.error !== null) {
    const detail = outcome.failedFiles > 0
      ? `${outcome.failedFiles} file${outcome.failedFiles === 1 ? "" : "s"} could not be deleted.`
      : outcome.cancelled
        ? "Deletion stopped safely."
        : (outcome.error ?? `${remainingItems} items remain.`);
    set({
      busy: false,
      commitFailure: {
        message: `${detail} Retry targets only the remaining items.`,
        permanent,
      },
    });
    return { kind: "failed" };
  }
  const family = state.sessionMembers;
  const spreadCount = get().spreadCount;
  set(closedComparisonState());
  await queueComparisonLifecycle(() => teardownComparison(spreadCount));
  return { kind: "completed", family };
}

function closedComparisonState(): Partial<ComparisonState> {
  return {
    open: false,
    members: [],
    kept: new Set(),
    visited: new Set(),
    page: 0,
    shortlist: false,
    shortlistPage: 0,
    sessionMembers: [],
    busy: false,
    permanentArmed: false,
    pendingPermanentCommit: false,
    pendingCommit: null,
    commitFailure: null,
    spreadCount: 0,
  };
}
