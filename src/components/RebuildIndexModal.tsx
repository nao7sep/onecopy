// Rebuild library index confirmation: the library index itself is always
// discarded (that is the point of the action), and provisional-key cache
// entries are always discarded with it regardless of these choices, because
// they name a path rather than content and cannot outlive the index row that
// gave them meaning. Previews/posters and transcripts are additional,
// separately reconstructible caches the user may also choose to discard;
// both default to kept, since discarding either only costs time to
// regenerate for no different a result.

import { useState } from "react";
import { useI18n } from "../i18n/I18nContext";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import { Row, Toggle } from "./ui/Field";

export default function RebuildIndexModal({
  onConfirm,
  onCancel,
}: {
  onConfirm: (options: { discardPreviews: boolean; discardTranscripts: boolean }) => void;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  const [discardPreviews, setDiscardPreviews] = useState(false);
  const [discardTranscripts, setDiscardTranscripts] = useState(false);

  return (
    <ModalShell
      title={t("settings.rebuildTitle")}
      onClose={onCancel}
      closeLabel={t("common.cancel")}
      initialFocus="close"
      footerArrowNavigation
      primaryAction={
        <Button
          variant="danger-solid"
          data-destructive
          onClick={() => onConfirm({ discardPreviews, discardTranscripts })}
        >
          {t("settings.rebuildConfirm")}
        </Button>
      }
    >
      <p className="text-sm text-ink">{t("settings.rebuildMessage")}</p>
      <p className="mt-3 text-xs font-semibold uppercase tracking-wide text-ink-muted">
        {t("settings.rebuildAlsoDiscard")}
      </p>
      <Row label={t("settings.rebuildDiscardPreviews")}>
        <Toggle checked={discardPreviews} onChange={setDiscardPreviews} />
      </Row>
      <Row label={t("settings.rebuildDiscardTranscripts")}>
        <Toggle checked={discardTranscripts} onChange={setDiscardTranscripts} />
      </Row>
      <p className="mt-2 rounded-md border border-warning/40 bg-warning-surface px-3 py-2 text-xs text-warning">
        {t("settings.rebuildTranscriptsWarning")}
      </p>
      <p className="mt-3 text-xs text-ink-muted">{t("confirm.hint")}</p>
    </ModalShell>
  );
}
