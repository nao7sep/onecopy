// Shared destructive confirmation: explicit Cancel-first focus and the
// approved footer-arrow exception live in the shell's input boundary.

import { useI18n } from "../i18n/I18nContext";
import ModalShell from "./ModalShell";

export default function ConfirmDialog({
  title,
  message,
  confirmLabel,
  cancelLabel,
  widthClass = "w-[400px]",
  onConfirm,
  onCancel,
}: {
  title: string;
  message: string;
  confirmLabel: string;
  /** The dismiss label, defaulting to the shared "Cancel" wording of the
   *  current language. Override where a more specific word reads better —
   *  "Keep editing" beside a Discard, for example. */
  cancelLabel?: string;
  widthClass?: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  return (
    <ModalShell
      title={title}
      onClose={onCancel}
      widthClass={widthClass}
      closeLabel={cancelLabel ?? t("common.cancel")}
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
      <p className="mt-2 text-xs text-ink-muted">{t("confirm.hint")}</p>
    </ModalShell>
  );
}
