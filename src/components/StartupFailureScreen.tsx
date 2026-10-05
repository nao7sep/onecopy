import { useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { useI18n } from "../i18n/I18nContext";
import { log, reportWindowCall, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";

/** The application-owned terminal bootstrap state. The webview is healthy, but
 * backend work is gated off because required application data is not. The
 * screen says why in the reader's language: required stores a newer OneCopy
 * wrote are named with their paths, and any other failure is one pair of
 * sentences whose diagnostic stays in the session log — reachable here, since
 * a fatal startup halt must name a safe next step AND provide access to the
 * logs when they can help (failures-and-recovery.md L61, Finding D). */
export default function StartupFailureScreen() {
  const { t } = useI18n();
  const newerStores = useAppStore((state) => state.startupFailure?.newerStores) ?? [];
  const [openLogsFailed, setOpenLogsFailed] = useState(false);
  const quit = () => {
    void getCurrentWindow().close().catch(reportWindowCall("startup quit"));
  };
  const openLogFolder = () => {
    setOpenLogsFailed(false);
    void invoke("reveal_data_subdir", { name: "logs" }).catch((error) => {
      log.warn("startup screen reveal logs failed", toErrorFields(error));
      setOpenLogsFailed(true);
    });
  };

  return (
    <div
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="startup-failure-title"
      aria-describedby="startup-failure-message"
      className="fixed inset-0 z-[1000] flex items-center justify-center bg-background p-8 text-ink"
    >
      <div className="w-full max-w-lg rounded-xl border border-border bg-surface p-8 shadow-xl">
        <h1 id="startup-failure-title" className="text-xl font-semibold text-ink-strong">
          {t("startup.blockedTitle")}
        </h1>
        <p id="startup-failure-message" className="mt-3 leading-relaxed text-ink-muted">
          {newerStores.length > 0 ? t("startup.newerBody") : t("startup.blockedBody")}
        </p>
        {newerStores.length > 0 ? (
          <ul className="mt-3 space-y-2">
            {newerStores.map((store) => (
              <li key={store.path} className="rounded-lg border border-border p-3">
                <p className="text-sm font-semibold text-ink-strong">{store.file}</p>
                <p className="mt-1 break-all text-xs text-ink-muted">{store.path}</p>
              </li>
            ))}
          </ul>
        ) : null}
        {openLogsFailed ? (
          <OperationResult level="error" className="mt-3 text-sm">
            {t("app.revealLogsFailed")}
          </OperationResult>
        ) : null}
        <div className="mt-6 flex justify-end gap-2">
          <Button onClick={openLogFolder}>{t("app.revealLogs")}</Button>
          <Button type="button" variant="primary" size="md" onClick={quit} autoFocus>
            {t("startup.quit")}
          </Button>
        </div>
      </div>
    </div>
  );
}
