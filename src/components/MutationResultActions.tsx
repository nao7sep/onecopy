import { X } from "lucide-react";
import { useI18n } from "../i18n/I18nContext";
import type { MutationResult } from "../models/mutation";
import { IconButton, LinkButton } from "./ui/Button";

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
  const { t } = useI18n();
  return (
    <span className="inline-flex items-center gap-2">
      {result.summary.trashAvailable ? (
        <LinkButton onClick={onRevealTrash}>
          {t("mutation.openDeletedFiles")}
        </LinkButton>
      ) : null}
      <IconButton
        size="sm"
        aria-label={t("mutation.dismissResult")}
        title={t("common.dismiss")}
        onClick={onDismiss}
      >
        <X size={14} strokeWidth={2} aria-hidden="true" />
      </IconButton>
    </span>
  );
}
