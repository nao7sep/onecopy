import { useEffect, useState } from "react";
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
  unlinkComparisonSelection,
} from "../workflows/comparison";
import ComparisonSlot from "./ComparisonSlot";
import ConfirmDialog from "./ConfirmDialog";
import RevealCopiesDialog from "./RevealCopiesDialog";
import OperationResult from "./ui/OperationResult";
import { X } from "lucide-react";

function interactiveTarget(target: EventTarget | null): boolean {
  return (
    target instanceof Element &&
    target.closest(
      "button, input, select, textarea, [contenteditable='true'], [role='menu']",
    ) !== null
  );
}

export default function ComparisonView() {
  const [revealMember, setRevealMember] = useState<{
    hash: string;
    fileName: string;
  } | null>(null);
  const open = useComparisonStore((state) => state.open);
  const members = useComparisonStore((state) => state.members);
  const page = useComparisonStore((state) => state.page);
  const maximumImages = useComparisonStore((state) => state.maximumImages);
  const displayCount = useComparisonStore((state) => state.displayCount);
  const spreadCount = useComparisonStore((state) => state.spreadCount);
  const portraitDominant = useComparisonStore(
    (state) => state.portraitDominant,
  );
  const pendingAction = useComparisonStore((state) => state.pendingAction);
  const failure = useComparisonStore((state) => state.failure);
  const message = useComparisonStore((state) => state.message);
  const busy = useComparisonStore((state) => state.busy);
  const mutationProgress = useMutationStore((state) => state.progress);
  const mutationCancelling = useMutationStore((state) => state.cancelling);
  const mutationResult = useMutationStore((state) => state.result);
  const exitQuiescing = useMutationStore((state) => state.exiting);
  const cancelMutation = useMutationStore((state) => state.cancel);
  const dismissMutationResult = useMutationStore((state) => state.dismissResult);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        hasOpenModal() ||
        isEditableTarget(event.target) ||
        interactiveTarget(event.target)
      ) {
        return;
      }
      if (handleComparisonKey(event)) event.preventDefault();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [open]);

  if (!open) return null;

  const state = useComparisonStore.getState();
  const pages = comparisonPages(members, maximumImages, displayCount);
  const chunks = comparisonChunks(state);
  const localChunk = chunks[0] ?? [];
  const grid = gridFor(
    localChunk.length,
    portraitDominant,
    window.innerWidth / Math.max(1, window.innerHeight),
  );
  const selectedCount = chunks
    .flat()
    .reduce((count, slot) => count + (slot.selected ? 1 : 0), 0);

  const confirmTitle = pendingAction?.permanent
    ? "Delete images permanently?"
    : pendingAction?.kind === "selection"
      ? "Move selected images to Trash?"
      : "Finish this comparison page?";
  const confirmMessage =
    pendingAction === null
      ? ""
      : pendingAction.kind === "selection"
        ? `${pendingAction.targetHashes.length} selected image${pendingAction.targetHashes.length === 1 ? "" : "s"} will be ${pendingAction.permanent ? "deleted permanently" : "moved to Trash"}.`
        : `Keep ${pendingAction.keepHashes.length} and ${pendingAction.permanent ? "permanently delete" : "move to Trash"} ${pendingAction.targetHashes.length} image${pendingAction.targetHashes.length === 1 ? "" : "s"} on this page.`;

  return (
    <div className="fixed inset-0 z-20 flex flex-col bg-background">
      {pendingAction !== null ? (
        <ConfirmDialog
          title={confirmTitle}
          message={confirmMessage}
          confirmLabel={
            pendingAction.permanent ? "Delete permanently" : "Move to Trash"
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
            undecided · {selectedCount} selected
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
            className="rounded border border-border px-2 py-1 text-ink hover:bg-surface-muted disabled:opacity-50"
            disabled={busy || selectedCount === 0}
            onClick={() => void unlinkComparisonSelection()}
          >
            Not similar
          </button>
          <button
            className="rounded border border-danger/50 px-2 py-1 text-danger hover:bg-danger/10 disabled:opacity-50"
            disabled={busy || localChunk.length === 0}
            onClick={() => void decideComparisonPage(false, true)}
          >
            Trash every visible image
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
        role="listbox"
        aria-label="Images on the current comparison page"
        aria-multiselectable="true"
        className="grid min-h-0 flex-1 grid-flow-col gap-3 p-3"
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
            selected={slot.selected}
            anchor={slot.anchor}
            onSelect={(mode) =>
              useComparisonStore.getState().selectSlot(index, mode)
            }
            onDecide={() => void decideComparisonPage(false)}
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
          0–9, A–Z toggle · Arrows select · Page Up/Down browse · Enter retains
          the selection and trashes the rest · Delete trashes the selection ·
          Escape closes
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
            <button
              className="inline-flex h-6 w-6 items-center justify-center rounded text-ink-muted transition-colors hover:bg-surface-muted hover:text-ink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary-ring"
              aria-label="Dismiss file-operation result"
              title="Dismiss"
              onClick={dismissMutationResult}
            >
              <X size={14} strokeWidth={2} aria-hidden="true" />
            </button>
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
