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
          className="rounded bg-danger-solid px-3 py-1 text-sm text-ink-inverted hover:bg-danger-solid-hover"
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
