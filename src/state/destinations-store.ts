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
  /** Targets nothing was written to at all — a full disk or unreadable
   * sources. Distinct from a conflict, and it withholds the post-action too. */
  undelivered: string[];
  postAction: { deletedFiles: number; failedFiles: number; removedRows: number };
}

export type MoveMode = "move-trash-rest" | "move-delete-rest" | "copy";

interface DestinationsState {
  roots: string[];
  children: Record<string, DirEntry[]>;
  expanded: Set<string>;
  emptiness: Record<string, boolean>;
  message: string;
  /** A move-delete-rest awaiting its permanent-deletion confirmation
   * (`confirmed` marks the re-entry pass so it is not re-staged).   * `keys` freezes the exact items the dialog counted, so a selection change
   * while it is open cannot redirect it. */
  pendingDeleteRest: {
    destDir: string;
    count: number;
    confirmed: boolean;
    keys: string[];
  } | null;
  confirmPendingDeleteRest: () => Promise<void>;
  cancelPendingDeleteRest: () => void;
  /** The tree's keyboard cursor (the composite-control active item). */
  activePath: string | null;
  setActive: (path: string | null) => void;
  /** A drop landed on this folder and awaits the Move/Copy choice (Phase 33:
   * a modal asks; modifier keys at drop time silently decided before). */
  pendingDrop: string | null;
  setPendingDrop: (path: string | null) => void;
  init: (config: Record<string, unknown> | null) => void;
  addRoot: () => Promise<void>;
  removeRoot: (root: string) => Promise<void>;
  toggleExpand: (path: string) => Promise<void>;
  refreshNode: (path: string) => Promise<void>;
  /** Re-lists every expanded node (and re-probes emptiness) — the pane calls
   * this on mount and when the app window regains focus, so a folder created
   * in Finder/Explorer appears without a restart. */
  refreshExpanded: () => Promise<void>;
  createFolder: (parent: string, name: string) => Promise<void>;
  deleteFolder: (path: string, parent: string) => Promise<void>;
  moveSelectionTo: (
    destDir: string,
    mode: MoveMode,
    /** Explicit targets, for a confirmed action that must act on the set it
     * quoted rather than whatever is selected now. */
    explicitKeys?: string[],
  ) => Promise<void>;
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

  pendingDrop: null,
  setPendingDrop: (path) => set({ pendingDrop: path }),

  pendingDeleteRest: null,

  confirmPendingDeleteRest: async () => {
    const pending = get().pendingDeleteRest;
    if (pending === null || pending.confirmed) return;
    // Mark confirmed so the re-entry bypasses staging; clear when done.
    set({ pendingDeleteRest: { ...pending, confirmed: true } });
    try {
      await get().moveSelectionTo(
        pending.destDir,
        "move-delete-rest",
        pending.keys,
      );
    } finally {
      set({ pendingDeleteRest: null });
    }
  },

  cancelPendingDeleteRest: () => set({ pendingDeleteRest: null }),

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

  moveSelectionTo: async (destDir, mode, explicitKeys) => {
    const { useItemsStore, itemKey } = await import("./items-store");
    const { items, selectedItem, selectedKeys } = useItemsStore.getState();
    // Fast feedback for the name-divergence block (the core refuses too, but
    // a whole multi-select should fail BEFORE any item moved): any selected
    // item whose copies carry different names stops the operation.
    {
      const keySet =
        explicitKeys !== undefined
          ? new Set(explicitKeys)
          : selectedKeys.size > 0
            ? selectedKeys
            : selectedItem !== null
              ? new Set([selectedItem])
              : new Set<string>();
      const blocked = items.filter((i) => keySet.has(itemKey(i)) && i.namesDiffer);
      if (blocked.length > 0) {
        set({
          message: `${blocked.length} selected item${blocked.length === 1 ? "" : "s"} have copies under different names — Move/Copy is disabled for them. Reveal the copies (Details) to resolve the names first.`,
        });
        return;
      }
    }
    // A confirmed permanent move acts on the set the dialog counted. Re-reading
    // the live selection here would let a click behind the dialog redirect it
    // — permanently destroying copies the user was never shown a count for.
    const keys =
      explicitKeys !== undefined
        ? new Set(explicitKeys)
        : selectedKeys.size > 0
          ? selectedKeys
          : selectedItem !== null
            ? new Set([selectedItem])
            : new Set<string>();
    // The delete-rest mode permanently destroys the remaining copies:
    // stage it behind the confirmation instead of running (the design's
    // permanent-always-confirms rule). The confirm action re-enters here
    // with the pending marker cleared.
    if (mode === "move-delete-rest" && !get().pendingDeleteRest?.confirmed && keys.size > 0) {
      set({
        pendingDeleteRest: {
          destDir,
          count: keys.size,
          confirmed: false,
          keys: [...keys],
        },
      });
      return;
    }
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
      const undelivered: string[] = [];
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
        undelivered.push(...outcome.undelivered);
      }
      const parts: string[] = [];
      if (exported > 0) parts.push(`${exported} exported`);
      if (skipped > 0) parts.push(`${skipped} already there`);
      if (handled > 0) parts.push(`${handled} originals handled`);
      if (conflicts.length > 0)
        parts.push(`CONFLICT: ${conflicts.join(", ")} differs — those items untouched`);
      if (undelivered.length > 0)
        parts.push(
          `FAILED: could not write ${undelivered.join(", ")} — those items untouched`,
        );
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
