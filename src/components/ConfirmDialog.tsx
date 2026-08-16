// The one confirmation surface for destructive commitments (permanent
// deletion): a ModalShell (so the stack, focus trap, and Escape semantics
// come for free) with a danger-styled primary and the shell's labelled
// dismiss beside it. Trash deletion never confirms — the trash is the safety
// net; this exists only where there is no net.

import ModalShell from "./ModalShell";

export default function ConfirmDialog({
  title,
  message,
  confirmLabel,
  onConfirm,
  onCancel,
}: {
  title: string;
  message: string;
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <ModalShell
      title={title}
      onClose={onCancel}
      widthClass="w-[400px]"
      closeLabel="Cancel"
      primaryAction={
        <button
          // Marks this surface as one whose primary action destroys data, so
          // the focus trap opens on Cancel instead of on this button.
          data-destructive
          className="inline-flex h-8 shrink-0 items-center justify-center rounded-lg bg-danger-solid px-3 text-sm font-medium text-ink-inverted shadow-sm outline-none transition-all hover:bg-danger-solid-hover focus-visible:ring-2 focus-visible:ring-primary-ring"
          onClick={onConfirm}
        >
          {confirmLabel}
        </button>
      }
    >
      <p className="text-sm text-ink">{message}</p>
    </ModalShell>
  );
}
