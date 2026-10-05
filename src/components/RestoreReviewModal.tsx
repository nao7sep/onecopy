// The restore review: shown only when something needs a decision or a
// warning (name conflicts, folders to recreate, skipped files, a companion
// that will not pair with its main file restored earlier under another name).
// It lists every selected file with where it goes. Restoring destroys
// nothing, so the primary action may take focus; Cancel does no filesystem
// work.

import type { MessageKey } from "../i18n/catalogues";
import { useI18n } from "../i18n/I18nContext";
import { reviewAction, type RestoreReview, type RestoreSkip } from "../models/deletedFiles";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";

const SKIP_REASONS: Record<RestoreSkip, MessageKey> = {
  "already-there": "restoreReview.skipAlreadyThere",
  changed: "deletedFiles.statusChanged",
  missing: "restoreReview.skipMissing",
  unrepresentable: "deletedFiles.statusUnrepresentable",
  excluded: "deletedFiles.statusExcluded",
  "folder-is-link": "restoreReview.skipFolderIsLink",
  "file-in-the-way": "restoreReview.skipFileInTheWay",
  "other-drive": "restoreReview.skipOtherDrive",
};

export default function RestoreReviewModal({
  review,
  changed,
  onCancel,
  onConfirm,
}: {
  review: RestoreReview;
  /** The plan differs from the one last confirmed. */
  changed: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useI18n();
  const action = reviewAction(review);
  return (
    <ModalShell
      title={t("restoreReview.title")}
      onClose={onCancel}
      closeLabel={t("common.cancel")}
      widthClass="w-[min(820px,calc(100vw-3rem))]"
      primaryAction={
        <Button variant="primary" disabled={action === "none"} onClick={onConfirm}>
          {action === "rename-and-restore"
            ? t("restoreReview.renameAndRestore")
            : t("deletedFiles.restore")}
        </Button>
      }
    >
      {changed ? (
        <p className="mb-3 text-sm text-warning">{t("restoreReview.changed")}</p>
      ) : null}
      <p className="mb-3 text-sm text-ink">{t("restoreReview.intro")}</p>
      {review.folders.length > 0 ? (
        <div className="mb-3 text-xs text-ink-muted">
          <p>{t("restoreReview.folders")}</p>
          <ul className="mt-1 list-inside list-disc">
            {review.folders.map((folder) => (
              <li key={folder} className="break-all">
                {folder}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      <div className="overflow-hidden rounded-lg border border-border">
        {review.files.map((file) => (
          <div key={file.id} className="border-b border-border px-3 py-2 last:border-b-0">
            <p className="break-all text-sm text-ink-strong">{file.original ?? file.id}</p>
            <p className="break-all text-xs text-ink-muted">
              {file.skip !== null
                ? t(SKIP_REASONS[file.skip])
                : file.renamed
                  ? t("restoreReview.renamed", { path: file.target ?? "" })
                  : t("restoreReview.target", { path: file.target ?? "" })}
            </p>
            {file.mainRestoredAs !== null ? (
              <p className="break-all text-xs text-warning">
                {t("restoreReview.companionUnpaired", { path: file.mainRestoredAs })}
              </p>
            ) : null}
          </div>
        ))}
      </div>
      {review.companionsLeft.length > 0 ? (
        <div className="mt-3 text-xs text-ink-muted">
          <p>{t("restoreReview.companionsLeft")}</p>
          <ul className="mt-1 list-inside list-disc">
            {review.companionsLeft.map((path) => (
              <li key={path} className="break-all">
                {path}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </ModalShell>
  );
}
