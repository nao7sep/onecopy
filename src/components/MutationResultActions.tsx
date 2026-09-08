import { X } from "lucide-react";
import type { MutationResult } from "../models/mutation";

/** The persistent operation receipt's remedies. Recovery availability comes
 * from the backend's completed operation, never from a frontend guess based
 * only on the broad mutation kind. */
export default function MutationResultActions({
  result,
  onRevealTrash,
  onDismiss,
}: {
  result: MutationResult;
  onRevealTrash: () => void;
  onDismiss: () => void;
}) {
  return (
    <span className="inline-flex items-center gap-2">
      {result.summary.trashAvailable ? (
        <button
          className="text-ink-muted hover:text-ink hover:underline"
          onClick={onRevealTrash}
        >
          Reveal deleted files…
        </button>
      ) : null}
      <button
        className="inline-flex h-6 w-6 items-center justify-center rounded text-ink-muted transition-colors hover:bg-surface-muted hover:text-ink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary-ring"
        aria-label="Dismiss file-operation result"
        title="Dismiss"
        onClick={onDismiss}
      >
        <X size={14} strokeWidth={2} aria-hidden="true" />
      </button>
    </span>
  );
}
