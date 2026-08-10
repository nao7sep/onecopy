// The destination tree: configured roots, lazily expanded subdirectories, and
// the move/copy-out actions over the grid's current selection. Destinations
// are configured outside the source directories (the core enforces it per
// operation); the tree is a destination panel, never a file manager — expand,
// create folder, delete EMPTY folder, move/copy here, nothing else.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { log, toErrorFields } from "../repositories";

export interface DirEntry {
  name: string;
  path: string;
  hasChildren: boolean;
}

export interface MoveOutOutcome {
  exported: number;
  skippedIdentical: number;
  conflicts: string[];
  postAction: { deletedFiles: number; failedFiles: number; removedRows: number };
}

export type MoveMode = "move-trash-rest" | "move-delete-rest" | "copy";

interface DestinationsState {
  roots: string[];
  children: Record<string, DirEntry[]>;
  expanded: Set<string>;
  emptiness: Record<string, boolean>;
  message: string;
  /** The tree's keyboard cursor (the composite-control active item). */
  activePath: string | null;
  setActive: (path: string | null) => void;
  init: (config: Record<string, unknown> | null) => void;
  addRoot: () => Promise<void>;
  removeRoot: (root: string) => Promise<void>;
  toggleExpand: (path: string) => Promise<void>;
  refreshNode: (path: string) => Promise<void>;
  createFolder: (parent: string, name: string) => Promise<void>;
  deleteFolder: (path: string, parent: string) => Promise<void>;
  moveSelectionTo: (destDir: string, mode: MoveMode) => Promise<void>;
}

async function probeEmptiness(paths: string[]): Promise<Record<string, boolean>> {
  const result: Record<string, boolean> = {};
  for (const path of paths) {
    try {
      result[path] = await invoke<boolean>("dir_is_empty", { path });
    } catch {
      result[path] = false;
    }
  }
  return result;
}

export const useDestinationsStore = create<DestinationsState>((set, get) => ({
  roots: [],
  children: {},
  expanded: new Set<string>(),
  emptiness: {},
  message: "",
  activePath: null,

  setActive: (path) => set({ activePath: path }),

  init: (config) => {
    const roots = Array.isArray(config?.destinationRoots)
      ? (config.destinationRoots as string[])
      : [];
    set({ roots });
  },

  addRoot: async () => {
    try {
      const picked = await openDialog({ directory: true, multiple: false });
      if (typeof picked !== "string") return;
      const { roots } = get();
      if (roots.includes(picked)) return;
      const next = [...roots, picked];
      // A patch of ONLY this key through the one config owner — a stale
      // cached copy elsewhere can no longer revert the added root.
      const { useAppStore } = await import("./app-store");
      await useAppStore.getState().patchConfig({ destinationRoots: next });
      set({ roots: next });
    } catch (error) {
      log.error("destination root add failed", toErrorFields(error));
    }
  },

  removeRoot: async (root) => {
    try {
      const next = get().roots.filter((r) => r !== root);
      const { useAppStore } = await import("./app-store");
      await useAppStore.getState().patchConfig({ destinationRoots: next });
      set({ roots: next });
    } catch (error) {
      log.error("destination root remove failed", toErrorFields(error));
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
    try {
      const entries = await invoke<DirEntry[]>("list_subdirs", { path });
      const emptiness = await probeEmptiness(entries.map((e) => e.path));
      set({
        children: { ...get().children, [path]: entries },
        emptiness: { ...get().emptiness, ...emptiness },
      });
    } catch (error) {
      log.error("destination listing failed", toErrorFields(error));
    }
  },

  createFolder: async (parent, name) => {
    try {
      await invoke<string>("create_subdir", { parent, name });
      await get().refreshNode(parent);
      set({ message: "" });
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

  moveSelectionTo: async (destDir, mode) => {
    const { useItemsStore, itemKey } = await import("./items-store");
    const { items, selectedItem, selectedKeys } = useItemsStore.getState();
    const keys =
      selectedKeys.size > 0
        ? selectedKeys
        : selectedItem !== null
          ? new Set([selectedItem])
          : new Set<string>();
    const targets = items.filter((i) => keys.has(itemKey(i)));
    if (targets.length === 0) {
      set({ message: "Select an item in the grid first" });
      return;
    }
    try {
      let exported = 0;
      let skipped = 0;
      let handled = 0;
      const conflicts: string[] = [];
      let done = 0;
      for (const item of targets) {
        done += 1;
        if (targets.length > 1) {
          set({ message: `Working… ${done}/${targets.length}` });
        }
        const outcome = await invoke<MoveOutOutcome>("move_item_out", {
          hash: item.hash,
          pathId: item.hash === null ? item.pathId : null,
          destDir,
          mode,
        });
        exported += outcome.exported;
        skipped += outcome.skippedIdentical;
        handled += outcome.postAction.deletedFiles;
        conflicts.push(...outcome.conflicts);
      }
      const parts: string[] = [];
      if (exported > 0) parts.push(`${exported} exported`);
      if (skipped > 0) parts.push(`${skipped} already there`);
      if (handled > 0) parts.push(`${handled} originals handled`);
      if (conflicts.length > 0)
        parts.push(`CONFLICT: ${conflicts.join(", ")} differs — those items untouched`);
      set({ message: parts.join(" · ") || "Nothing to do" });
      await useItemsStore.getState().refresh();
      const { useSectionsStore } = await import("./sections-store");
      await useSectionsStore.getState().loadCounts();
    } catch (error) {
      set({ message: String(error) });
      log.error("move out failed", toErrorFields(error));
    }
  },
}));
