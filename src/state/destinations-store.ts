// The destination tree's own state and filesystem adapters. Config persistence
// and move/copy-out journeys live in workflows/destinations because they span
// app, item, and section owners.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";
import { stringArrayField } from "../utils/configProjection";
import type {
  DestinationDragPresentation,
  DestinationSelection,
  PendingDestinationDrop,
} from "../models/destinationTransfer";

export interface DirEntry {
  name: string;
  path: string;
  hasChildren: boolean;
  isEmpty: boolean;
}

export interface DestinationResult {
  severity: "info" | "warning" | "error";
  message: string;
  /** Identifies the receiver action whose committed outcome this describes.
   * A successful retry clears only a result with the same operation key. */
  operationKey: string;
}

interface DestinationsState {
  roots: string[];
  children: Record<string, DirEntry[]>;
  listing: Record<string, "loading" | "error">;
  expanded: Set<string>;
  emptiness: Record<string, boolean>;
  message: string;
  result: DestinationResult | null;
  dismissResult: () => void;
  /** A clean Copy needs local confirmation because its source row remains and
   * destination files are not rendered in this tree.  It is independent from
   * an older unresolved result, which an unrelated success must not erase. */
  confirmation: string | null;
  dismissConfirmation: () => void;
  /** A move-delete-rest awaiting its permanent-deletion confirmation. The
   * backend identities freeze exactly what the dialog counted, independent of
   * later selection or watcher projection changes. */
  pendingDeleteRest: {
    destDir: string;
    count: number;
    selection: DestinationSelection;
  } | null;
  cancelPendingDeleteRest: () => void;
  /** The tree's keyboard cursor (the composite-control active item). */
  activePath: string | null;
  setActive: (path: string | null) => void;
  /** Exact selected identities carried by the current internal drag. */
  dragSelection: DestinationSelection | null;
  setDragSelection: (selection: DestinationSelection | null) => void;
  /** Semantic receiver currently under the app-owned pointer drag. */
  dragReceiverPath: string | null;
  setDragReceiverPath: (path: string | null) => void;
  /** Pointer-following payload preview; presentation only, never authority. */
  dragPresentation: DestinationDragPresentation | null;
  /** A drop landed and awaits the Move/Copy choice. Both receiver and dragged
   * identities are frozen; the modal never rereads live grid selection. */
  pendingDrop: PendingDestinationDrop | null;
  setPendingDrop: (drop: PendingDestinationDrop | null) => void;
  init: (config: Record<string, unknown> | null) => void;
  toggleExpand: (path: string) => Promise<void>;
  refreshNode: (path: string) => Promise<void>;
  /** Re-lists every expanded node — the pane calls
   * this on mount and when the app window regains focus, so a folder created
   * in Finder/Explorer appears without a restart. */
  refreshExpanded: () => Promise<void>;
  createFolder: (parent: string, name: string) => Promise<void>;
  deleteFolder: (path: string, parent: string) => Promise<void>;
}

export const useDestinationsStore = create<DestinationsState>((set, get) => ({
  roots: [],
  children: {},
  listing: {},
  expanded: new Set<string>(),
  emptiness: {},
  message: "",
  result: null,
  dismissResult: () => set({ result: null }),
  confirmation: null,
  dismissConfirmation: () => set({ confirmation: null }),
  activePath: null,

  setActive: (path) => set({ activePath: path }),

  dragSelection: null,
  setDragSelection: (selection) => set({ dragSelection: selection }),
  dragReceiverPath: null,
  setDragReceiverPath: (path) => set({ dragReceiverPath: path }),
  dragPresentation: null,

  pendingDrop: null,
  setPendingDrop: (drop) => set({ pendingDrop: drop }),

  pendingDeleteRest: null,

  cancelPendingDeleteRest: () => set({ pendingDeleteRest: null }),

  init: (config) => {
    const roots = stringArrayField(config, "destinationRoots");
    set({ roots });
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
    set({ listing: { ...get().listing, [path]: "loading" } });
    try {
      const entries = await invoke<DirEntry[]>("list_subdirs", { path });
      const emptiness = Object.fromEntries(entries.map((entry) => [entry.path, entry.isEmpty]));
      const listing = { ...get().listing };
      delete listing[path];
      set({
        children: { ...get().children, [path]: entries },
        emptiness: { ...get().emptiness, ...emptiness },
        listing,
      });
    } catch (error) {
      log.error("destination listing failed", toErrorFields(error));
      set({ listing: { ...get().listing, [path]: "error" } });
    }
  },

  refreshExpanded: async () => {
    // Everything expanded — roots and subdirectories alike — re-lists; a
    // collapsed node re-lists on its next expand anyway.
    for (const path of get().expanded) {
      await get().refreshNode(path);
    }
  },

  createFolder: async (parent, name) => {
    try {
      const created = await invoke<string>("create_subdir", { parent, name });
      await get().refreshNode(parent);
      // The new folder must be VISIBLE and active immediately — before this,
      // a subfolder made under a collapsed leaf existed only on disk (the
      // leaf's stale hasChildren said there was nothing to expand) and the
      // developer read the feature as broken.
      set({
        expanded: new Set(get().expanded).add(parent),
        activePath: created,
        message: "",
      });
    } catch (error) {
      set({ message: String(error) });
    }
  },

  deleteFolder: async (path, parent) => {
    try {
      await invoke("delete_empty_dir", { path });
      await get().refreshNode(parent);
      set({ message: "" });
    } catch (error) {
      set({ message: String(error) });
    }
  },

}));
