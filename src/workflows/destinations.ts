// Destination journeys at the application edge. The destination store owns
// tree state and direct folder adapters; this module coordinates config
// persistence and item export with the item and section projections.

import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { log, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import { useDestinationsStore } from "../state/destinations-store";
import { useIssuesStore } from "../state/issues-store";
import { itemKey, useItemsStore } from "../state/items-store";
import { useSectionsStore } from "../state/sections-store";

interface MoveBatchOutcome {
  cancelled: boolean;
  error: string | null;
  exported: number;
  skippedIdentical: number;
  conflicts: string[];
  undelivered: string[];
  postAction: { deletedFiles: number; failedFiles: number };
}

interface ItemIdentity {
  hash: string | null;
  pathId: number | null;
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
  if (pending === null) return;
  // The modal relinquishes its frozen intent before admission. A second click
  // therefore cannot submit the same permanent batch twice; once admitted,
  // cancellation belongs to the shared mutation activity in the footer.
  useDestinationsStore.setState({ pendingDeleteRest: null });
  await executeMoveBatch(pending.destDir, "move-delete-rest", pending.items);
}

export async function moveSelectionTo(
  destDir: string,
  mode: MoveMode,
): Promise<void> {
  const { items, selectedItem, selectedKeys } = useItemsStore.getState();
  const keys =
    selectedKeys.size > 0
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
  const targets = items.filter((item) => keys.has(itemKey(item)));
  if (targets.length === 0) {
    useDestinationsStore.setState({
      message: "Select an item in the grid first",
    });
    return;
  }
  const identities = targets.map((item) => ({
    hash: item.hash,
    pathId: item.hash === null ? item.pathId : null,
  }));
  if (mode === "move-delete-rest") {
    useDestinationsStore.setState({
      pendingDeleteRest: {
        destDir,
        count: identities.length,
        items: identities,
      },
    });
    return;
  }
  await executeMoveBatch(destDir, mode, identities);
}

async function executeMoveBatch(
  destDir: string,
  mode: MoveMode,
  identities: ItemIdentity[],
): Promise<void> {
  try {
    const outcome = await invoke<MoveBatchOutcome>("move_items_out", {
      items: identities,
      destDir,
      mode,
    });
    const parts: string[] = [];
    if (outcome.exported > 0) parts.push(`${outcome.exported} exported`);
    if (outcome.skippedIdentical > 0) {
      parts.push(`${outcome.skippedIdentical} already there`);
    }
    if (outcome.postAction.deletedFiles > 0) {
      parts.push(`${outcome.postAction.deletedFiles} originals handled`);
    }
    if (outcome.postAction.failedFiles > 0) {
      parts.push(
        `FAILED: ${outcome.postAction.failedFiles} originals could not be handled — see Issues`,
      );
    }
    if (outcome.conflicts.length > 0) {
      parts.push(
        `CONFLICT: ${outcome.conflicts.join(", ")} differs — originals kept`,
      );
    }
    if (outcome.undelivered.length > 0) {
      parts.push(
        `FAILED: could not write ${outcome.undelivered.join(", ")} — originals kept`,
      );
    }
    if (outcome.cancelled) parts.push("Stopped — unstarted items untouched");
    if (outcome.error !== null) parts.push(`STOPPED: ${outcome.error}`);
    useDestinationsStore.setState({
      message: parts.join(" · ") || "Nothing to do",
    });
    await refreshDestinationOwners();
  } catch (error) {
    useDestinationsStore.setState({ message: String(error) });
    log.error("move out failed", toErrorFields(error));
    await refreshDestinationOwners();
  }
}

async function refreshDestinationOwners(): Promise<void> {
  await Promise.all([
    useItemsStore.getState().refresh(),
    useSectionsStore.getState().loadCounts(),
    useIssuesStore.getState().load(),
  ]);
}
