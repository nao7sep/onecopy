// Shared destructive confirmation: explicit Cancel-first focus and the
// approved footer-arrow exception live in the shell's input boundary.

import { useI18n } from "../i18n/I18nContext";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";

export default function ConfirmModal({
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
        <Button
          variant="danger-solid"
          data-destructive
          onClick={onConfirm}
        >
          {confirmLabel}
        </Button>
      }
    >
      <p className="text-sm text-ink">{message}</p>
      <p className="mt-2 text-xs text-ink-muted">{t("confirm.hint")}</p>
    </ModalShell>
  );
}
