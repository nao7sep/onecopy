// Shared destructive confirmation: explicit Cancel-first focus and the
// approved footer-arrow exception live in the shell's input boundary.

import ModalShell from "./ModalShell";

export default function ConfirmDialog({
  title,
  message,
  confirmLabel,
  cancelLabel = "Cancel",
  widthClass = "w-[400px]",
  onConfirm,
  onCancel,
}: {
  title: string;
  message: string;
  confirmLabel: string;
  /** The dismiss label. Override where a more specific word reads better than
   *  the default — "Keep editing" beside a Discard, for example. */
  cancelLabel?: string;
  widthClass?: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <ModalShell
      title={title}
      onClose={onCancel}
      widthClass={widthClass}
      closeLabel={cancelLabel}
      initialFocus="close"
      footerArrowNavigation
      primaryAction={
        <button
          data-destructive
          className="inline-flex h-8 shrink-0 items-center justify-center rounded-lg bg-danger-solid px-3 text-sm font-medium text-ink-inverted shadow-sm outline-none transition-all hover:bg-danger-solid-hover focus:ring-2 focus:ring-primary-ring"
          onClick={onConfirm}
        >
          {confirmLabel}
        </button>
      }
    >
      <p className="text-sm text-ink">{message}</p>
      <p className="mt-2 text-xs text-ink-muted">Left/Right or Tab: choose · Enter: confirm choice · Escape: cancel</p>
    </ModalShell>
  );
}
