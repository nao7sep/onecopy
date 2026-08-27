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
import { sortItems } from "../models/items";
import type {
  DestinationItemIdentity,
  DestinationSelection,
} from "../models/destinationTransfer";

interface MoveBatchOutcome {
  cancelled: boolean;
  error: string | null;
  exported: number;
  skippedIdentical: number;
  conflicts: string[];
  undelivered: string[];
  postAction: { deletedFiles: number; failedFiles: number };
}

export type MoveMode = "move-trash-rest" | "move-delete-rest" | "copy";

function receiverOperationKey(destDir: string, mode: MoveMode): string {
  return JSON.stringify([destDir, mode]);
}

function moveOperationKey(
  destDir: string,
  mode: MoveMode,
  identities: readonly DestinationItemIdentity[],
): string {
  const items = identities
    .map((item) => (item.hash !== null ? `hash:${item.hash}` : `path:${item.pathId}`))
    .sort();
  return JSON.stringify([destDir, mode, items]);
}

function destinationLabel(path: string): string {
  const trimmed = path.replace(/[/\\]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut >= 0 ? trimmed.slice(cut + 1) || trimmed : trimmed;
}

/** Captures the exact logical items and source order for one Move/Copy intent. */
export function captureDestinationSelection(): DestinationSelection {
  const state = useItemsStore.getState();
  const keys =
    state.selectedKeys.size > 0
      ? state.selectedKeys
      : state.selectedItem !== null
        ? new Set([state.selectedItem])
        : new Set<string>();
  const targets = state.items.filter((item) => keys.has(itemKey(item)));
  return {
    items: targets.map((item) => ({
      hash: item.hash,
      pathId: item.hash === null ? item.pathId : null,
    })),
    blockedNameCount: targets.filter((item) => item.namesDiffer).length,
    anchorKey: state.selectedItem,
    shownKeys: sortItems(state.items, state.currentSort()).map(itemKey),
  };
}

/** Begins one app-owned internal drag. The immutable store snapshot is the
 * authority and prevents an external or synthetic browser payload from
 * becoming a OneCopy operation. */
export function beginDestinationDrag(key: string): DestinationSelection | null {
  const state = useItemsStore.getState();
  if (!state.selectedKeys.has(key)) state.selectItem(key);
  const selection = captureDestinationSelection();
  if (selection.items.length === 0) return null;
  useDestinationsStore.setState({
    dragSelection: selection,
    dragReceiverPath: null,
    dragPresentation: null,
  });
  log.debug("destination drag started", { items: selection.items.length });
  return selection;
}

/** Ends the interaction and returns the frozen intent to a receiver, if any. */
export function takeDestinationDrag(): DestinationSelection | null {
  const selection = useDestinationsStore.getState().dragSelection;
  useDestinationsStore.setState({
    dragSelection: null,
    dragReceiverPath: null,
    dragPresentation: null,
  });
  if (selection !== null) {
    log.debug("destination drag received", { items: selection.items.length });
  }
  return selection;
}

export function cancelDestinationDrag(): void {
  if (useDestinationsStore.getState().dragSelection !== null) {
    log.debug("destination drag cancelled");
  }
  useDestinationsStore.setState({
    dragSelection: null,
    dragReceiverPath: null,
    dragPresentation: null,
  });
}

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
  await executeMoveBatch(pending.destDir, "move-delete-rest", pending.selection);
}

export async function moveSelectionTo(
  destDir: string,
  mode: MoveMode,
): Promise<void> {
  await moveDestinationSelectionTo(destDir, mode, captureDestinationSelection());
}

/** Shared admission and durable-operation boundary for drag, buttons, and
 * keyboard commands. Callers may capture at different moments, but nobody
 * reinterprets an already-captured selection. */
export async function moveDestinationSelectionTo(
  destDir: string,
  mode: MoveMode,
  selection: DestinationSelection,
): Promise<void> {
  const operationKey = moveOperationKey(destDir, mode, selection.items);
  useDestinationsStore.setState({ confirmation: null });
  // Fail the whole multi-selection before moving anything if copy names
  // disagree. The core enforces the same rule per operation.
  if (selection.blockedNameCount > 0) {
    useDestinationsStore.setState({
      result: {
        severity: "warning",
        message: `${selection.blockedNameCount} selected item${selection.blockedNameCount === 1 ? "" : "s"} have copies under different names — Move/Copy is disabled for them. Reveal the copies (Details) to resolve the names first.`,
        operationKey,
      },
    });
    return;
  }
  if (selection.items.length === 0) {
    useDestinationsStore.setState({
      result: {
        severity: "warning",
        message: "Select an item in the grid first.",
        operationKey: receiverOperationKey(destDir, mode),
      },
    });
    return;
  }
  if (mode === "move-delete-rest") {
    useDestinationsStore.setState({
      pendingDeleteRest: {
        destDir,
        count: selection.items.length,
        selection,
      },
    });
    return;
  }
  await executeMoveBatch(destDir, mode, selection);
}

async function executeMoveBatch(
  destDir: string,
  mode: MoveMode,
  selection: DestinationSelection,
): Promise<void> {
  const operationKey = moveOperationKey(destDir, mode, selection.items);
  const receiverKey = receiverOperationKey(destDir, mode);
  let operationCompleted = false;
  try {
    const outcome = await invoke<MoveBatchOutcome>("move_items_out", {
      items: selection.items,
      destDir,
      mode,
    });
    operationCompleted = true;
    const parts: string[] = [];
    const hasCommittedNonSuccess =
      outcome.skippedIdentical > 0 ||
      outcome.postAction.failedFiles > 0 ||
      outcome.conflicts.length > 0 ||
      outcome.undelivered.length > 0 ||
      outcome.cancelled ||
      outcome.error !== null;
    // A partial/cancelled outcome accounts for successful work as well as
    // every refusal. A clean success stays out of this result surface.
    if (hasCommittedNonSuccess && outcome.exported > 0) {
      parts.push(
        `${outcome.exported} file${outcome.exported === 1 ? "" : "s"} delivered`,
      );
    }
    if (hasCommittedNonSuccess && outcome.postAction.deletedFiles > 0) {
      parts.push(
        `${outcome.postAction.deletedFiles} original${outcome.postAction.deletedFiles === 1 ? "" : "s"} handled`,
      );
    }
    if (outcome.skippedIdentical > 0) {
      parts.push(
        `${outcome.skippedIdentical} file${outcome.skippedIdentical === 1 ? " is" : "s are"} already there`,
      );
    }
    if (outcome.postAction.failedFiles > 0) {
      parts.push(
        `${outcome.postAction.failedFiles} original${outcome.postAction.failedFiles === 1 ? "" : "s"} could not be handled — see Issues`,
      );
    }
    if (outcome.conflicts.length > 0) {
      parts.push(
        `${outcome.conflicts.length} destination file${outcome.conflicts.length === 1 ? "" : "s"} already ${outcome.conflicts.length === 1 ? "exists" : "exist"} with different content; originals kept: ${outcome.conflicts.join(", ")}`,
      );
    }
    if (outcome.undelivered.length > 0) {
      parts.push(
        `Could not write ${outcome.undelivered.join(", ")}; originals kept`,
      );
    }
    if (outcome.cancelled) parts.push("Stopped; unstarted items are untouched");
    if (outcome.error !== null) parts.push(`Stopped: ${outcome.error}`);
    if (
      parts.length === 0 &&
      outcome.exported === 0 &&
      outcome.postAction.deletedFiles === 0
    ) {
      parts.push("Nothing changed");
    }
    const severity =
      outcome.postAction.failedFiles > 0 ||
      outcome.undelivered.length > 0 ||
      outcome.error !== null
        ? "error"
        : outcome.conflicts.length > 0
          ? "warning"
          : "info";
    if (parts.length > 0) {
      useDestinationsStore.setState({
        result: { severity, message: `${parts.join(" · ")}.`, operationKey },
        confirmation: null,
      });
    } else {
      const state = useDestinationsStore.getState();
      const corrected =
        state.result?.operationKey === operationKey ||
        state.result?.operationKey === receiverKey;
      useDestinationsStore.setState({
        ...(corrected ? { result: null } : {}),
        confirmation:
          mode === "copy"
            ? `Copied ${outcome.exported} file${outcome.exported === 1 ? "" : "s"} to ${destinationLabel(destDir)}.`
            : null,
      });
    }
  } catch (error) {
    useDestinationsStore.setState({
      result: {
        severity: "error",
        message: String(error),
        operationKey,
      },
      confirmation: null,
    });
    log.error("move out failed", toErrorFields(error));
  }

  try {
    await refreshDestinationOwners(selection);
  } catch (error) {
    log.error("destination projections refresh failed", toErrorFields(error));
    useDestinationsStore.setState({
      result: {
        severity: "error",
        message: operationCompleted
          ? "The file operation finished, but OneCopy could not refresh its view. Reopen or rescan before continuing."
          : "OneCopy could not refresh after the failed file operation. Reopen or rescan before continuing.",
        operationKey,
      },
      confirmation: null,
    });
  }
}

async function refreshDestinationOwners(
  selection: DestinationSelection,
): Promise<void> {
  await Promise.all([
    useItemsStore.getState().refresh(),
    useSectionsStore.getState().loadCounts(),
    useIssuesStore.getState().load(),
    useDestinationsStore.getState().refreshExpanded(),
  ]);

  // A Move can remove the active source row. Recover to the next surviving
  // item in the drag-start order, then the previous one, without overriding a
  // different surviving item the user selected while the operation ran.
  const state = useItemsStore.getState();
  if (state.selectedItem !== null || selection.anchorKey === null) return;
  const requested = new Set(
    selection.items.map((item) =>
      item.hash !== null ? item.hash : `path-${item.pathId}`,
    ),
  );
  if (!requested.has(selection.anchorKey)) return;
  const alive = new Set(state.items.map(itemKey));
  const anchorIndex = selection.shownKeys.indexOf(selection.anchorKey);
  const start = Math.max(anchorIndex, 0);
  const orderedSurvivor = (allowed: Set<string>) =>
    selection.shownKeys.slice(start).find((key) => allowed.has(key)) ??
    [...selection.shownKeys.slice(0, start)]
      .reverse()
      .find((key) => allowed.has(key)) ??
    null;
  const selectedSurvivor = orderedSurvivor(state.selectedKeys);
  if (selectedSurvivor !== null) {
    state.setAnchor(selectedSurvivor);
  } else {
    state.selectItem(orderedSurvivor(alive));
  }
}
