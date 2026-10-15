import { useEffect } from "react";
import { useI18n } from "../i18n/I18nContext";
import { useQuitRunningWorkStore } from "../workflows/quit";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";

/** Quitting over a running file operation or managed-tool install: Keep
 * running is the default and cancels the quit. */
export default function QuitRunningWorkModal() {
  const choose = useQuitRunningWorkStore((state) => state.choose);
  const presented = useQuitRunningWorkStore((state) => state.presented);
  const standalone = useQuitRunningWorkStore((state) => state.standalone);
  const body = useQuitRunningWorkStore((state) => state.body);
  useEffect(() => { presented?.(); }, [presented]);
  const { t, text } = useI18n();
  if (choose === null || standalone) return null;
  return <ModalShell title={t("quit.runningWorkTitle")} onClose={() => choose(false)}
    closeLabel={t("quit.keepRunning")} initialFocus="close" footerArrowNavigation
    primaryAction={<Button variant="danger-solid" data-destructive onClick={() => choose(true)}>{t("quit.stopAndQuit")}</Button>}>
    {body === null ? null : <p className="whitespace-pre-line text-sm text-ink">{text(body)}</p>}
  </ModalShell>;
}
