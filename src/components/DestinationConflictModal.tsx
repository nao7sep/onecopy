import type { PendingDestinationConflicts } from "../models/destinationTransfer";
import { formatBytes } from "../models/items";
import { useDestinationsStore } from "../state/destinations-store";
import { resolveDestinationConflicts } from "../workflows/destinations";
import { useI18n } from "../i18n/I18nContext";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";

function fileName(path: string): string {
  const trimmed = path.replace(/[/\\]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut < 0 ? trimmed : trimmed.slice(cut + 1);
}

export default function DestinationConflictModal({
  pending,
}: {
  pending: PendingDestinationConflicts;
}) {
  const { t, number } = useI18n();
  const close = () => useDestinationsStore.getState().setPendingConflicts(null);
  const copying = pending.mode === "copy";
  return (
    <ModalShell
      title={copying ? t("conflict.copyTitle") : t("conflict.moveTitle")}
      onClose={close}
      closeLabel={t("common.cancel")}
      initialFocus="close"
      footerArrowNavigation
      widthClass="w-[min(760px,calc(100vw-3rem))]"
      primaryAction={
        <>
          <Button
            variant="primary"
            onClick={() => void resolveDestinationConflicts("rename")}
          >
            {copying ? t("conflict.renameAndCopy") : t("conflict.renameAndMove")}
          </Button>
          <Button
            variant="danger"
            disabled={!pending.overwriteAllowed}
            title={
              pending.overwriteAllowed
                ? t("conflict.overwriteHint")
                : t("conflict.overwriteBlockedHint")
            }
            onClick={() => void resolveDestinationConflicts("overwrite")}
          >
            {t("conflict.overwrite")}
          </Button>
        </>
      }
    >
      <p className="text-sm text-ink">{t("conflict.intro")}</p>
      <div className="mt-3 overflow-hidden border border-border">
        {pending.conflicts.map((conflict, index) => (
          <div
            key={`${conflict.path}-${index}`}
            className="border-b border-border px-3 py-2 last:border-b-0"
          >
            <p className="break-all text-sm font-medium text-ink-strong">
              {fileName(conflict.path)}
            </p>
            <p className="break-all text-xs text-ink-muted">{conflict.path}</p>
            <p className="mt-1 text-xs text-ink-muted">
              {conflict.withinSelection
                ? t("conflict.incomingWithinSelection", {
                    size: formatBytes(conflict.incomingBytes, number),
                  })
                : conflict.existingBytes === null
                  ? t("conflict.incomingExistingNotRegular", {
                      size: formatBytes(conflict.incomingBytes, number),
                    })
                  : t("conflict.incomingExisting", {
                      size: formatBytes(conflict.incomingBytes, number),
                      existingSize: formatBytes(conflict.existingBytes, number),
                    })}
            </p>
            {conflict.preservedPaths.length > 1 ? (
              <div className="mt-2 text-xs text-ink-muted">
                <p>{t("conflict.preservedFamily")}</p>
                <ul className="mt-1 list-inside list-disc">
                  {conflict.preservedPaths.map((path) => (
                    <li key={path} className="break-all">
                      {path}
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}
          </div>
        ))}
      </div>
      {!pending.overwriteAllowed ? (
        <p className="mt-3 text-xs text-warning">{t("conflict.renameRequired")}</p>
      ) : (
        <p className="mt-3 text-xs text-ink-muted">{t("conflict.overwriteNote")}</p>
      )}
    </ModalShell>
  );
}
