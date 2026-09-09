import { useEffect, useRef, useState } from "react";
import { isComposingEvent } from "../hooks/useComposing";
import { useComparisonLayout } from "../hooks/useComparisonLayout";
import { comparisonPages, gridFor } from "../models/comparisonSession";
import { mutationProgressLine, mutationResultLine } from "../models/mutation";
import {
  comparisonChunks,
  useComparisonStore,
} from "../state/comparison-store";
import { useMutationStore } from "../state/mutation-store";
import { hasOpenModal } from "../utils/modalStack";
import { isEditableTarget } from "../utils/shortcuts";
import {
  closeComparison,
  confirmComparisonAction,
  decideComparisonPage,
  handleComparisonKey,
  retryComparisonFailure,
} from "../workflows/comparison";
import ComparisonSlot from "./ComparisonSlot";
import ConfirmDialog from "./ConfirmDialog";
import RevealCopiesDialog from "./RevealCopiesDialog";
import OperationResult from "./ui/OperationResult";
import MutationResultActions from "./MutationResultActions";

function interactiveTarget(target: EventTarget | null): boolean {
  return (
    target instanceof Element &&
    target.closest(
      "button, input, select, textarea, [contenteditable='true'], [role='menu']",
    ) !== null
  );
}

export default function ComparisonView({
  onRevealTrash,
}: {
  onRevealTrash: () => void;
}) {
  const itemArea = useRef<HTMLDivElement>(null);
  const [revealMember, setRevealMember] = useState<{
    hash: string;
    fileName: string;
  } | null>(null);
  // Cards, marks, inspection and page controls render the same subscribed
  // snapshot. An imperative read here can silently omit a render dependency.
  const state = useComparisonStore();
  const {
    open, members, page, maximumImages, displayCount, spreadCount,
    portraitDominant, pendingAction, failure, message, busy,
  } = state;
  const mutationProgress = useMutationStore((state) => state.progress);
  const mutationCancelling = useMutationStore((state) => state.cancelling);
  const mutationResult = useMutationStore((state) => state.result);
  const exitQuiescing = useMutationStore((state) => state.exiting);
  const cancelMutation = useMutationStore((state) => state.cancel);
  const dismissMutationResult = useMutationStore((state) => state.dismissResult);
  useComparisonLayout(itemArea, open, (aspect) => useComparisonStore.getState().setDisplayAspect(0, aspect));

  useEffect(() => {
    if (!open) return;
    itemArea.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        isComposingEvent(event) ||
        hasOpenModal() ||
        isEditableTarget(event.target) ||
        interactiveTarget(event.target)
      ) {
        return;
      }
      if (handleComparisonKey(event)) {
        event.preventDefault();
        event.stopPropagation();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [open]);

  if (!open) return null;

  const pages = comparisonPages(members, maximumImages, displayCount);
  const chunks = comparisonChunks(state);
  const localChunk = chunks[0] ?? [];
  const grid = gridFor(
    localChunk.length,
    portraitDominant,
    state.displayAspects[0],
  );
  const markedCount = chunks
    .flat()
    .reduce((count, slot) => count + (slot.marked ? 1 : 0), 0);

  const confirmTitle = pendingAction?.permanent
    ? "Delete images permanently?"
    : pendingAction?.kind === "selection"
      ? "Delete marked images?"
      : "Finish this comparison page?";
  const confirmMessage =
    pendingAction === null
      ? ""
      : pendingAction.kind === "selection"
        ? `${pendingAction.targetHashes.length} marked image${pendingAction.targetHashes.length === 1 ? "" : "s"} will be ${pendingAction.permanent ? "deleted permanently" : "deleted recoverably"}.`
        : `Keep ${pendingAction.keepHashes.length} and ${pendingAction.permanent ? "permanently delete" : "recoverably delete"} ${pendingAction.targetHashes.length} image${pendingAction.targetHashes.length === 1 ? "" : "s"} on this page.`;

  return (
    <div className="fixed inset-0 z-20 flex flex-col bg-background">
      {pendingAction !== null ? (
        <ConfirmDialog
          title={confirmTitle}
          message={confirmMessage}
          confirmLabel={
            pendingAction.permanent ? "Delete permanently" : "Delete"
          }
          onConfirm={() => void confirmComparisonAction()}
          onCancel={() => useComparisonStore.getState().cancelPendingAction()}
        />
      ) : null}
      {revealMember !== null ? (
        <RevealCopiesDialog
          hash={revealMember.hash}
          fileName={revealMember.fileName}
          onClose={() => setRevealMember(null)}
        />
      ) : null}

      <header className="flex shrink-0 items-center justify-between gap-4 border-b border-border bg-surface px-3 py-2">
        <div className="min-w-0">
          <h1 className="text-sm font-semibold text-ink-strong">
            Similar images
          </h1>
          <p className="text-xs text-ink-muted">
            Page {page + 1}/{Math.max(1, pages.length)} · {members.length}{" "}
            undecided · {markedCount} marked to keep
            {spreadCount > 0 ? ` · ${spreadCount + 1} displays` : ""}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2 text-xs">
          <button
            className="rounded border border-border px-2 py-1 text-ink hover:bg-surface-muted disabled:opacity-50"
            disabled={busy || page <= 0}
            onClick={() => useComparisonStore.getState().prevPage()}
          >
            Previous Page
          </button>
          <button
            className="rounded border border-border px-2 py-1 text-ink hover:bg-surface-muted disabled:opacity-50"
            disabled={busy || page >= pages.length - 1}
            onClick={() => useComparisonStore.getState().nextPage()}
          >
            Next Page
          </button>
          <button
            className="rounded border border-danger/50 px-2 py-1 text-danger hover:bg-danger/10 disabled:opacity-50"
            disabled={busy || localChunk.length === 0}
            onClick={() => void decideComparisonPage(false, true)}
          >
            Delete every visible image
          </button>
          <button
            className="rounded border border-border px-2 py-1 text-ink hover:bg-surface-muted disabled:opacity-50"
            disabled={busy}
            onClick={() => void closeComparison()}
          >
            Close
          </button>
        </div>
      </header>

      <div
        id="comparison-item-area"
        ref={itemArea}
        tabIndex={0}
        role="listbox"
        aria-label="Images on the current comparison page"
        aria-multiselectable="true"
        className="grid min-h-0 flex-1 grid-flow-row gap-3 p-3"
        style={{
          gridTemplateColumns: `repeat(${grid.columns}, minmax(0, 1fr))`,
          gridTemplateRows: `repeat(${grid.rows}, minmax(0, 1fr))`,
        }}
      >
        {localChunk.map((slot, index) => (
          <ComparisonSlot
            key={slot.member.hash}
            member={slot.member}
            slotKey={slot.slotKey}
            marked={slot.marked}
            anchor={slot.anchor}
            onSelect={(mode) => {
              itemArea.current?.focus();
              useComparisonStore.getState().selectSlot(index, mode);
            }}
            onReveal={() =>
              setRevealMember({
                hash: slot.member.hash,
                fileName: slot.member.fileName,
              })
            }
          />
        ))}
      </div>

      <footer className="flex shrink-0 items-center justify-between gap-4 border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
        <span>
          0–9, A–Z, or Keep toggle marks · Space opens the picked image · Arrows inspect · Page Up/Down browse · Enter reviews marked keepers and visible deletions ·
          Delete reviews marked images · Escape closes
        </span>
        {message !== null ? (
          <span className="text-warning">{message}</span>
        ) : null}
      </footer>

      {busy || mutationResult !== null || exitQuiescing ? (
        <footer className="flex shrink-0 items-center justify-between gap-3 border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
          <span>
            {exitQuiescing
              ? "Finishing current file before exit…"
              : mutationProgress !== null
                ? mutationProgressLine(mutationProgress, mutationCancelling)
                : mutationResult !== null
                  ? mutationResultLine(mutationResult)
                  : "Preparing file operation…"}
          </span>
          {mutationProgress !== null && !exitQuiescing ? (
            <button
              className="rounded border border-border px-2 py-0.5 text-ink hover:bg-surface-muted disabled:opacity-50"
              disabled={mutationCancelling}
              onClick={() => void cancelMutation()}
            >
              {mutationCancelling ? "Cancelling…" : "Cancel file operation"}
            </button>
          ) : mutationResult !== null && !exitQuiescing ? (
            <MutationResultActions
              result={mutationResult}
              onRevealTrash={onRevealTrash}
              onDismiss={dismissMutationResult}
            />
          ) : null}
        </footer>
      ) : null}

      {failure !== null ? (
        <OperationResult
          level="error"
          className="mx-3 mb-2 shrink-0"
          actions={
            <button
              className="rounded border border-danger/40 px-2 py-0.5 hover:bg-danger/10"
              onClick={() => void retryComparisonFailure()}
            >
              Retry remaining
            </button>
          }
        >
          {failure.message}
        </OperationResult>
      ) : null}
    </div>
  );
}
