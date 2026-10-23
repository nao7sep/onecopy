import { displayNativePath } from "../utils/nativePath";
import { useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { useI18n } from "../i18n/I18nContext";
import { log, reportWindowCall, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";
import BoundedText from "./ui/BoundedText";

/** A blocked shell retains safe recovery and an explicit diagnostic view,
 * independently of ordinary stores and application commands. */
export default function StartupFailureScreen() {
  const { t } = useI18n();
  const failure = useAppStore((state) => state.startupFailure);
  const newerStores = failure?.newerStores ?? [];
  const [showDiagnostics, setShowDiagnostics] = useState(false);
  const [openFolderFailed, setOpenFolderFailed] = useState(false);
  const [protecting, setProtecting] = useState(false);
  const [protectedFolder, setProtectedFolder] = useState(false);
  const [protectionFailure, setProtectionFailure] = useState<string | null>(null);
  const makePrivate = () => {
    setProtecting(true);
    setProtectionFailure(null);
    void invoke("protect_startup_data_folder").then(() => setProtectedFolder(true), (error) => {
      log.warn("startup data folder protection failed", toErrorFields(error));
      setProtectionFailure(String(error));
    }).finally(() => setProtecting(false));
  };
  const quit = () => {
    void getCurrentWindow().close().catch(reportWindowCall("startup quit"));
  };
  const openDataFolder = () => {
    setOpenFolderFailed(false);
    void invoke("reveal_startup_data_folder").catch((error) => {
      log.warn("startup screen reveal data folder failed", toErrorFields(error));
      setOpenFolderFailed(true);
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
      <div className="max-h-full w-full max-w-lg overflow-y-auto rounded-xl border border-border bg-surface p-8 shadow-xl">
        <h1 id="startup-failure-title" className="text-xl font-semibold text-ink-strong">
          {t("startup.blockedTitle")}
        </h1>
        <p id="startup-failure-message" className="mt-3 leading-relaxed text-ink-muted">
          {t(failure?.privacyRequired ? "startup.privateBody" : newerStores.length > 0 ? "startup.newerBody" : "startup.blockedBody")}
        </p>
        {newerStores.length > 0 ? (
          // Each store's path is read whole; the list scrolls inside a
          // height limit so the actions below stay in view.
          <BoundedText className="mt-3 max-h-64">
            <ul className="space-y-2">
              {newerStores.map((store) => (
                <li key={store.path} className="rounded-lg border border-border p-3">
                  <p className="text-sm font-semibold text-ink-strong">{displayNativePath(store.file)}</p>
                  <p className="mt-1 break-all text-xs text-ink-muted">{displayNativePath(store.path)}</p>
                </li>
              ))}
            </ul>
          </BoundedText>
        ) : null}
        {showDiagnostics ? (
          <BoundedText className="mt-3 max-h-64">
            {failure?.diagnostic ? <pre className="whitespace-pre-wrap break-words text-xs">{failure.diagnostic}</pre> : null}
            {protectionFailure !== null ? <pre className="mt-2 whitespace-pre-wrap break-words text-xs">{protectionFailure}</pre> : null}
            {failure?.dataRoot ? <p className="mt-2 break-all text-xs">{displayNativePath(failure.dataRoot)}</p> : null}
          </BoundedText>
        ) : null}
        {protecting ? <p role="status" className="mt-3 text-sm">{t("startup.privateWorking")}</p> : null}
        {protectedFolder ? <OperationResult level="info" className="mt-3 text-sm">{t("startup.privateReady")}</OperationResult> : null}
        {protectionFailure !== null ? <OperationResult level="error" className="mt-3 text-sm">{t("startup.privateFailed")}</OperationResult> : null}
        {openFolderFailed ? (
          <OperationResult level="error" className="mt-3 text-sm">
            {t("startup.dataFolderFailed")}
          </OperationResult>
        ) : null}
        <div className="mt-6 flex flex-wrap justify-end gap-2">
          {failure?.diagnostic || failure?.dataRoot ? (
            <Button onClick={() => setShowDiagnostics(!showDiagnostics)} aria-expanded={showDiagnostics}>{t("records.details")}</Button>
          ) : null}
          {showDiagnostics && failure?.dataRoot ? <Button onClick={openDataFolder}>{t("startup.revealDataFolder")}</Button> : null}
          {failure?.privacyRequired && !protectedFolder ? <Button onClick={makePrivate} disabled={protecting}>{t("startup.makePrivate")}</Button> : null}
          <Button type="button" variant="primary" size="md" onClick={quit} autoFocus>
            {t("startup.quit")}
          </Button>
        </div>
      </div>
    </div>
  );
}
