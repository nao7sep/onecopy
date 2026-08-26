// Destination journeys at the application edge. The destination store owns
// tree state and direct folder adapters; this module coordinates config
// persistence and item export with the item and section projections.

import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { log, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import { useDestinationsStore } from "../state/destinations-store";
import { itemKey, useItemsStore } from "../state/items-store";
import { useSectionsStore } from "../state/sections-store";

interface MoveOutOutcome {
  exported: number;
  skippedIdentical: number;
  conflicts: string[];
  undelivered: string[];
  postAction: { deletedFiles: number };
}

export type MoveMode = "move-trash-rest" | "move-delete-rest" | "copy";

export async function addDestinationRoot(): Promise<void> {
  try {
    const picked = await openDialog({ directory: true, multiple: false });
    if (typeof picked !== "string") return;
    const roots = useDestinationsStore.getState().roots;
    if (roots.includes(picked)) return;
    const next = [...roots, picked];
    await useAppStore.getState().patchConfig({ destinationRoots: next });
    useDestinationsStore.setState({ roots: next });
  } catch (error) {
    log.error("destination root add failed", toErrorFields(error));
  }
}

export async function removeDestinationRoot(root: string): Promise<void> {
  try {
    const next = useDestinationsStore
      .getState()
      .roots.filter((candidate) => candidate !== root);
    await useAppStore.getState().patchConfig({ destinationRoots: next });
    useDestinationsStore.setState({ roots: next });
  } catch (error) {
    log.error("destination root remove failed", toErrorFields(error));
  }
}

export async function confirmDestinationDeleteRest(): Promise<void> {
  const pending = useDestinationsStore.getState().pendingDeleteRest;
  if (pending === null || pending.confirmed) return;
  useDestinationsStore.setState({
    pendingDeleteRest: { ...pending, confirmed: true },
  });
  try {
    await moveSelectionTo(
      pending.destDir,
      "move-delete-rest",
      pending.keys,
    );
  } finally {
    useDestinationsStore.setState({ pendingDeleteRest: null });
  }
}

export async function moveSelectionTo(
  destDir: string,
  mode: MoveMode,
  explicitKeys?: string[],
): Promise<void> {
  const { items, selectedItem, selectedKeys } = useItemsStore.getState();
  // A confirmed permanent move acts on the exact set the dialog counted.
  const keys =
    explicitKeys !== undefined
      ? new Set(explicitKeys)
      : selectedKeys.size > 0
        ? selectedKeys
        : selectedItem !== null
          ? new Set([selectedItem])
          : new Set<string>();
  // Fail the whole multi-selection before moving anything if copy names
  // disagree. The core enforces the same rule per operation.
  const blocked = items.filter(
    (item) => keys.has(itemKey(item)) && item.namesDiffer,
  );
  if (blocked.length > 0) {
    useDestinationsStore.setState({
      message: `${blocked.length} selected item${blocked.length === 1 ? "" : "s"} have copies under different names — Move/Copy is disabled for them. Reveal the copies (Details) to resolve the names first.`,
    });
    return;
  }
  if (
    mode === "move-delete-rest" &&
    !useDestinationsStore.getState().pendingDeleteRest?.confirmed &&
    keys.size > 0
  ) {
    useDestinationsStore.setState({
      pendingDeleteRest: {
        destDir,
        count: keys.size,
        confirmed: false,
        keys: [...keys],
      },
    });
    return;
  }
  const targets = items.filter((item) => keys.has(itemKey(item)));
  if (targets.length === 0) {
    useDestinationsStore.setState({
      message: "Select an item in the grid first",
    });
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
        useDestinationsStore.setState({
          message: `Working… ${done}/${targets.length}`,
        });
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
    if (conflicts.length > 0) {
      parts.push(
        `CONFLICT: ${conflicts.join(", ")} differs — those items untouched`,
      );
    }
    if (undelivered.length > 0) {
      parts.push(
        `FAILED: could not write ${undelivered.join(", ")} — those items untouched`,
      );
    }
    useDestinationsStore.setState({
      message: parts.join(" · ") || "Nothing to do",
    });
    await useItemsStore.getState().refresh();
    await useSectionsStore.getState().loadCounts();
  } catch (error) {
    useDestinationsStore.setState({ message: String(error) });
    log.error("move out failed", toErrorFields(error));
  }
}
