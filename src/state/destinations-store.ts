// The destination tree's own state and filesystem adapters. Config persistence
// and move/copy-out journeys live in workflows/destinations because they span
// app, item, and section owners.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { message, type Message } from "../i18n/translate";
import { log, toErrorFields } from "../repositories";
import { stringArrayField } from "../utils/configProjection";
import type {
  DestinationSelection,
  PendingDestinationDrop,
  PendingDestinationConflicts,
} from "../models/destinationTransfer";
import { recordActionFailure } from "./notifications-store";
import { recordActivity } from "../repositories/activity";

export interface DirEntry {
  name: string;
  path: string;
  hasChildren: boolean;
  isEmpty: boolean;
}

export interface DirListing {
  entries: DirEntry[];
  isEmpty: boolean;
}

/** One fact of a destination outcome. A fact about several files also keeps
 * their paths: its `names` value is the whole list as text, and the receiver
 * names a few of `paths` and counts the rest in its place
 * (interface-styling-conventions, "A message names a few values"). */
export type DestinationFact = Message & { paths?: readonly string[] };

export interface DestinationResult {
  severity: "info" | "warning" | "error";
  /** The facts this outcome reports, each a whole translated unit. The receiver
   * joins and terminates them at render, so the sentence follows a language
   * change after the operation has finished. */
  facts: DestinationFact[];
  /** Identifies the receiver action whose committed outcome this describes.
   * A successful retry clears only a result with the same operation key. */
  operationKey: string;
}

let nextListingGeneration = 0;
const listingGenerations = new Map<string, number>();
let visibilityGeneration = 0;

function isInSubtree(candidate: string, path: string): boolean {
  return candidate === path || candidate.startsWith(`${path}/`) || candidate.startsWith(`${path}\\`);
}

interface DestinationsState {
  roots: string[];
  children: Record<string, DirEntry[]>;
  listing: Record<string, "loading" | "error">;
  expanded: Set<string>;
  emptiness: Record<string, boolean>;
  message: Message | null;
  result: DestinationResult | null;
  dismissResult: () => void;
  /** A clean Copy needs local confirmation because its source row remains and
   * destination files are not rendered in this tree.  It is independent from
   * an older unresolved result, which an unrelated success must not erase. */
  confirmation: Message | null;
  dismissConfirmation: () => void;
  /** A Move awaiting its source-cleanup confirmation. The
   * backend identities freeze exactly what the dialog counted, independent of
   * later selection or watcher projection changes. */
  pendingMove: {
    destDir: string;
    count: number;
    mode: "move-trash-rest" | "move-delete-rest";
    selection: DestinationSelection;
  } | null;
  cancelPendingMove: () => void;
  /** The tree's keyboard cursor (the composite-control active item). */
  activePath: string | null;
  setActive: (path: string | null) => void;
  /** Exact selected identities carried by the current internal drag. */
  dragSelection: DestinationSelection | null;
  /** A drop landed and awaits the Move/Copy choice. Both receiver and dragged
   * identities are frozen; the modal never rereads live grid selection. */
  pendingDrop: PendingDestinationDrop | null;
  setPendingDrop: (drop: PendingDestinationDrop | null) => void;
  pendingConflicts: PendingDestinationConflicts | null;
  setPendingConflicts: (conflicts: PendingDestinationConflicts | null) => void;
  init: (config: Record<string, unknown> | null) => void;
  toggleExpand: (path: string) => Promise<void>;
  refreshNode: (path: string) => Promise<void>;
  /** Re-lists every expanded node — the pane calls
   * this on mount and when the app window regains focus, so a folder created
   * in Finder/Explorer appears without a restart. */
  refreshExpanded: () => Promise<void>;
  reconcileVisibility: () => Promise<void>;
  createFolder: (parent: string, name: string) => Promise<void>;
  deleteFolder: (path: string, parent: string) => Promise<void>;
}

export const useDestinationsStore = create<DestinationsState>((set, get) => ({
  roots: [],
  children: {},
  listing: {},
  // The expanded folders and the active destination last for this run only.
  expanded: new Set<string>(),
  emptiness: {},
  message: null,
  result: null,
  dismissResult: () => set({ result: null }),
  confirmation: null,
  dismissConfirmation: () => set({ confirmation: null }),
  activePath: null,

  setActive: (path) => set({ activePath: path }),

  dragSelection: null,

  pendingDrop: null,
  setPendingDrop: (drop) => set({ pendingDrop: drop }),

  pendingConflicts: null,
  setPendingConflicts: (conflicts) => set({ pendingConflicts: conflicts }),

  pendingMove: null,

  cancelPendingMove: () => set({ pendingMove: null }),

  init: (config) => {
    const roots = stringArrayField(config, "destinationRoots");
    if (JSON.stringify(roots) !== JSON.stringify(get().roots)) {
      visibilityGeneration++;
      listingGenerations.clear();
      set({ roots, listing: {} });
    }
  },

  toggleExpand: async (path) => {
    const { expanded } = get();
    const next = new Set(expanded);
    if (next.has(path)) {
      next.delete(path);
      set({ expanded: next });
      return;
    }
    next.add(path);
    set({ expanded: next });
    await get().refreshNode(path);
  },

  refreshNode: async (path) => {
    const generation = ++nextListingGeneration;
    listingGenerations.set(path, generation);
    set({ listing: { ...get().listing, [path]: "loading" } });
    try {
      const { entries, isEmpty } = await invoke<DirListing>("list_subdirs", { path });
      if (listingGenerations.get(path) !== generation) {
        recordActivity({
          kind: "stale",
          owner: "destination",
          generation,
          current: "stale",
          reason: "staleResponse",
        });
        return;
      }
      // A child refreshed directly after this read began owns its newer fact.
      const emptiness = Object.fromEntries(entries
        .filter((entry) => (listingGenerations.get(entry.path) ?? 0) <= generation)
        .map((entry) => [entry.path, entry.isEmpty]));
      const listing = { ...get().listing };
      delete listing[path];
      set({
        children: { ...get().children, [path]: entries },
        emptiness: { ...get().emptiness, ...emptiness, [path]: isEmpty },
        listing,
      });
    } catch (error) {
      if (listingGenerations.get(path) !== generation) {
        recordActivity({
          kind: "stale",
          owner: "destination",
          generation,
          current: "stale",
          reason: "staleResponse",
        });
        return;
      }
      log.error("destination listing failed", toErrorFields(error));
      set({ listing: { ...get().listing, [path]: "error" } });
      recordActionFailure(
        "destination-list-failed",
        message("destinations.listFailed"),
        error,
      );
    }
  },

  refreshExpanded: async () => {
    // Everything expanded — roots and subdirectories alike — re-lists; a
    // collapsed node re-lists on its next expand anyway.
    for (const path of get().expanded) {
      await get().refreshNode(path);
    }
  },

  reconcileVisibility: async () => {
    const generation = ++visibilityGeneration;
    const before = get();
    listingGenerations.clear();
    set({ children: {}, listing: {}, emptiness: {}, expanded: new Set(), activePath: null });
    const reachable = new Set(before.roots);
    const visit = async (path: string) => {
      if (generation !== visibilityGeneration) return;
      await get().refreshNode(path);
      if (generation !== visibilityGeneration) return;
      for (const child of get().children[path] ?? []) {
        reachable.add(child.path);
        if (before.expanded.has(child.path) || before.children[child.path]) await visit(child.path);
      }
    };
    for (const root of before.roots) await visit(root);
    if (generation !== visibilityGeneration) return;
    set({
      expanded: new Set([...get().expanded, ...[...before.expanded].filter((path) => reachable.has(path))]),
      activePath: get().activePath ?? (before.activePath !== null && reachable.has(before.activePath) ? before.activePath : null),
    });
  },

  createFolder: async (parent, name) => {
    const generation = visibilityGeneration;
    const activePath = get().activePath;
    try {
      const created = await invoke<string>("create_subdir", { parent, name });
      if (generation !== visibilityGeneration) return;
      await get().refreshNode(parent);
      if (generation !== visibilityGeneration) return;
      // The new folder must be VISIBLE and active immediately — before this,
      // a subfolder made under a collapsed leaf existed only on disk (the
      // leaf's stale hasChildren said there was nothing to expand) and the
      // developer read the feature as broken.
      set({
        expanded: new Set(get().expanded).add(parent),
        activePath: get().activePath === activePath ? created : get().activePath,
        message: null,
      });
    } catch (error) {
      log.error("destination folder creation failed", toErrorFields(error));
      set({ message: message("destinations.folderCreateFailed") });
      recordActionFailure(
        "destination-folder-create-failed",
        message("destinations.folderCreateFailedNotice"),
        error,
      );
    }
  },

  deleteFolder: async (path, parent) => {
    const generation = visibilityGeneration;
    const activePath = get().activePath;
    try {
      await invoke("delete_empty_dir", { path });
      if (generation !== visibilityGeneration) return;
      for (const key of listingGenerations.keys()) {
        if (isInSubtree(key, path)) listingGenerations.delete(key);
      }
      const current = get();
      const outsideDeleted = <T,>(values: Record<string, T>) => Object.fromEntries(
        Object.entries(values).filter(([key]) => !isInSubtree(key, path)),
      );
      set({
        children: outsideDeleted(current.children),
        emptiness: outsideDeleted(current.emptiness),
        listing: outsideDeleted(current.listing),
        expanded: new Set([...current.expanded].filter((key) => !isInSubtree(key, path))),
        activePath: current.activePath === activePath && activePath !== null && isInSubtree(activePath, path)
          ? parent : current.activePath,
        message: null,
      });
      await get().refreshNode(parent);
    } catch (error) {
      log.error("destination folder deletion failed", toErrorFields(error));
      set({ message: message("destinations.folderDeleteFailed") });
      recordActionFailure(
        "destination-folder-delete-failed",
        message("destinations.folderDeleteFailedNotice"),
        error,
      );
    }
  },

}));
