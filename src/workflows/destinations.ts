// Destination journeys at the application edge. The destination store owns
// tree state and direct folder adapters; this module coordinates config
// persistence and item export with the item and section projections.

import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { message, type Message } from "../i18n/translate";
import { log, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import { useDestinationsStore } from "../state/destinations-store";
import { useIssuesStore } from "../state/issues-store";
import { useItemsStore } from "../state/items-store";
import { useSectionsStore } from "../state/sections-store";
import { identityFromKey } from "../models/items";
import type {
  DestinationItemIdentity,
  DestinationSelection,
  DestinationConflict,
} from "../models/destinationTransfer";
import { recordActionFailure } from "../state/notifications-store";

interface MoveBatchOutcome {
  cancelled: boolean;
  error: string | null;
  exported: number;
  skippedIdentical: number;
  conflicts: string[];
  undelivered: string[];
  postAction: { deletedFiles: number; failedFiles: number };
  planToken: string | null;
  requiresConflictChoice: boolean;
  planChanged: boolean;
  overwriteAllowed: boolean;
  reviewedConflicts: DestinationConflict[];
}

export type MoveMode = "move-trash-rest" | "move-delete-rest" | "copy";
export type DestinationConflictPolicy = "rename" | "overwrite";

let destinationRootTail: Promise<void> = Promise.resolve();

function enqueueDestinationRootChange(task: () => Promise<void>): Promise<void> {
  const operation = destinationRootTail.then(task, task);
  destinationRootTail = operation.catch(() => undefined);
  return operation;
}

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
  const orderedKeys = [...keys].sort(
    (left, right) =>
      (state.selectedPositions.get(left) ?? Number.MAX_SAFE_INTEGER) -
      (state.selectedPositions.get(right) ?? Number.MAX_SAFE_INTEGER),
  );
  return {
    items: orderedKeys.map((key) => {
      const identity = identityFromKey(key);
      return {
        hash: identity.hash,
        pathId: identity.hash === null ? identity.pathId : null,
      };
    }),
    anchorKey: state.selectedItem,
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
  });
  log.debug("destination drag started", { items: selection.items.length });
  return selection;
}

/** Ends the interaction and returns the frozen intent to a receiver, if any. */
export function takeDestinationDrag(): DestinationSelection | null {
  const selection = useDestinationsStore.getState().dragSelection;
  useDestinationsStore.setState({
    dragSelection: null,
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
  });
}

export async function addDestinationRoot(): Promise<void> {
  try {
    const picked = await openDialog({ directory: true, multiple: false });
    if (typeof picked !== "string") return;
    await enqueueDestinationRootChange(async () => {
      const roots = useDestinationsStore.getState().roots;
      if (roots.includes(picked)) return;
      const next = [...roots, picked];
      await useAppStore
        .getState()
        .patchConfig({ destinationRoots: next }, { reportFailure: false });
      useDestinationsStore.setState({ roots: next, message: null });
    });
  } catch (error) {
    log.error("destination root add failed", toErrorFields(error));
    const failure = message("destinations.addFailed");
    useDestinationsStore.setState({ message: failure });
    recordActionFailure("destination-add-failed", failure, error);
  }
}

export async function removeDestinationRoot(root: string): Promise<void> {
  try {
    await enqueueDestinationRootChange(async () => {
      const next = useDestinationsStore
        .getState()
        .roots.filter((candidate) => candidate !== root);
      await useAppStore
        .getState()
        .patchConfig({ destinationRoots: next }, { reportFailure: false });
      useDestinationsStore.setState({ roots: next, message: null });
    });
  } catch (error) {
    log.error("destination root remove failed", toErrorFields(error));
    const failure = message("destinations.removeFailed");
    useDestinationsStore.setState({ message: failure });
    recordActionFailure("destination-remove-failed", failure, error);
  }
}

export async function confirmDestinationMove(): Promise<void> {
  const pending = useDestinationsStore.getState().pendingMove;
  if (pending === null) return;
  // The modal relinquishes its frozen intent before admission. A second click
  // therefore cannot submit the same batch twice; once admitted,
  // cancellation belongs to the shared mutation activity in the footer.
  useDestinationsStore.setState({ pendingMove: null });
  await executeMoveBatch(pending.destDir, pending.mode, pending.selection);
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
  useDestinationsStore.setState({ confirmation: null });
  if (selection.items.length === 0) {
    useDestinationsStore.setState({
      result: {
        severity: "warning",
        facts: [message("destinations.selectItemFirst")],
        operationKey: receiverOperationKey(destDir, mode),
      },
    });
    return;
  }
  if (mode !== "copy") {
    useDestinationsStore.setState({
      pendingMove: {
        destDir,
        count: selection.items.length,
        mode,
        selection,
      },
    });
    return;
  }
  await executeMoveBatch(destDir, mode, selection);
}

/** Consumes the one frozen drop whose visible choice modal already reviewed
 * its Move/Trash consequence. Clearing the pending drop before admission both
 * prevents a double submission and keeps this from becoming a general bypass. */
export async function acceptDestinationDropChoice(
  mode: "move-trash-rest" | "copy",
): Promise<void> {
  const pending = useDestinationsStore.getState().pendingDrop;
  if (pending === null) return;
  const { path, selection } = pending;
  useDestinationsStore.setState({
    pendingDrop: null,
    activePath: path,
    confirmation: null,
  });
  if (selection.items.length === 0) {
    useDestinationsStore.setState({
      result: {
        severity: "warning",
        facts: [message("destinations.selectItemFirst")],
        operationKey: receiverOperationKey(path, mode),
      },
    });
    return;
  }
  await executeMoveBatch(path, mode, selection);
}

async function executeMoveBatch(
  destDir: string,
  mode: MoveMode,
  selection: DestinationSelection,
  conflictPolicy: DestinationConflictPolicy | null = null,
  planToken: string | null = null,
): Promise<void> {
  const operationKey = moveOperationKey(destDir, mode, selection.items);
  const receiverKey = receiverOperationKey(destDir, mode);
  let operationCompleted = false;
  try {
    const outcome = await invoke<MoveBatchOutcome>("move_items_out", {
      items: selection.items,
      destDir,
      mode,
      conflictPolicy,
      planToken,
    });
    if (outcome.planChanged) {
      useDestinationsStore.setState({
        result: {
          severity: "warning",
          facts: [message("destinations.planChanged")],
          operationKey,
        },
        confirmation: null,
      });
      return;
    }
    if (outcome.requiresConflictChoice && outcome.planToken !== null) {
      useDestinationsStore.setState({
        pendingConflicts: {
          destDir,
          mode,
          selection,
          planToken: outcome.planToken,
          conflicts: outcome.reviewedConflicts,
          overwriteAllowed: outcome.overwriteAllowed,
        },
        confirmation: null,
      });
      return;
    }
    operationCompleted = true;
    const facts: Message[] = [];
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
      facts.push(message("destinations.factDelivered", { count: outcome.exported }));
    }
    if (hasCommittedNonSuccess && outcome.postAction.deletedFiles > 0) {
      facts.push(
        message("destinations.factOriginalsHandled", {
          count: outcome.postAction.deletedFiles,
        }),
      );
    }
    if (outcome.skippedIdentical > 0) {
      facts.push(
        message("destinations.factAlreadyThere", { count: outcome.skippedIdentical }),
      );
    }
    if (outcome.postAction.failedFiles > 0) {
      facts.push(
        message("destinations.factOriginalsFailed", {
          count: outcome.postAction.failedFiles,
        }),
      );
    }
    if (outcome.conflicts.length > 0) {
      facts.push(
        message("destinations.factConflicts", {
          count: outcome.conflicts.length,
          names: outcome.conflicts.join(", "),
        }),
      );
    }
    if (outcome.undelivered.length > 0) {
      facts.push(
        message("destinations.factUndelivered", {
          names: outcome.undelivered.join(", "),
        }),
      );
    }
    if (outcome.cancelled) facts.push(message("destinations.factCancelled"));
    if (outcome.error !== null) {
      facts.push(message("destinations.factStopped"));
    }
    if (
      facts.length === 0 &&
      outcome.exported === 0 &&
      outcome.postAction.deletedFiles === 0
    ) {
      facts.push(message("destinations.factNothingChanged"));
    }
    const severity =
      outcome.postAction.failedFiles > 0 ||
      outcome.undelivered.length > 0 ||
      outcome.error !== null
        ? "error"
        : outcome.conflicts.length > 0
          ? "warning"
          : "info";
    if (facts.length > 0) {
      useDestinationsStore.setState({
        result: { severity, facts, operationKey },
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
            ? message("destinations.copiedToFolder", {
                count: outcome.exported,
                dest: destinationLabel(destDir),
              })
            : null,
      });
    }
  } catch (error) {
    useDestinationsStore.setState({
      result: {
        severity: "error",
        facts: [message("destinations.operationFailed")],
        operationKey,
      },
      confirmation: null,
    });
    recordActionFailure(
      "destination-operation-failed",
      message("destinations.operationFailedNotice"),
      error,
    );
    log.error("move out failed", toErrorFields(error));
  }

  try {
    await refreshDestinationOwners();
  } catch (error) {
    log.error("destination projections refresh failed", toErrorFields(error));
    useDestinationsStore.setState({
      result: {
        severity: "error",
        facts: [
          message(
            operationCompleted
              ? "destinations.refreshFailedAfterSuccess"
              : "destinations.refreshFailedAfterFailure",
          ),
        ],
        operationKey,
      },
      confirmation: null,
    });
    recordActionFailure(
      "destination-refresh-failed",
      message(
        operationCompleted
          ? "destinations.refreshFailedAfterSuccessNotice"
          : "destinations.refreshFailedAfterFailureNotice",
      ),
      error,
    );
  }
}

export async function resolveDestinationConflicts(
  policy: DestinationConflictPolicy,
): Promise<void> {
  const pending = useDestinationsStore.getState().pendingConflicts;
  if (pending === null) return;
  useDestinationsStore.setState({ pendingConflicts: null });
  await executeMoveBatch(
    pending.destDir,
    pending.mode,
    pending.selection,
    policy,
    pending.planToken,
  );
}

async function refreshDestinationOwners(): Promise<void> {
  await Promise.all([
    useItemsStore.getState().refresh(),
    useSectionsStore.getState().loadCounts(),
    useIssuesStore.getState().load(),
    useDestinationsStore.getState().refreshExpanded(),
  ]);

  // Main refresh owns next/previous recovery from its bounded anchor context.
  // Keeping a second drag-start copy of the section order would be both
  // unbounded and a competing selection authority.
}
