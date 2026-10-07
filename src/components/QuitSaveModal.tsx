import { useEffect } from "react";
import { useI18n } from "../i18n/I18nContext";
import { useQuitSaveStore } from "../workflows/quit";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";

export default function QuitSaveModal() {
  const choose = useQuitSaveStore((state) => state.choose);
  const presented = useQuitSaveStore((state) => state.presented);
  useEffect(() => { presented?.(); }, [presented]);
  const { t } = useI18n();
  if (choose === null) return null;
  return <ModalShell title={t("quit.saveFailedTitle")} onClose={() => choose("cancel")}
    closeLabel={t("common.cancel")} initialFocus="close" footerArrowNavigation
    primaryAction={<><Button onClick={() => choose("retry")}>{t("common.retry")}</Button>
      <Button variant="danger-solid" data-destructive onClick={() => choose("quit")}>{t("quit.anyway")}</Button></>}>
    <p className="text-sm text-ink">{t("quit.saveFailedBody")}</p>
  </ModalShell>;
}
