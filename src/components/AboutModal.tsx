// The About surface (modal-dialog conventions' required payload): name,
// version, one-line description, repository/issues links, copyright, license.

import { useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { message } from "../i18n/translate";
import { log, toErrorFields } from "../repositories";
import { recordActionFailure } from "../state/notifications-store";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";
import {
  LATEST_RELEASE_PAGE,
  useReleaseCheckStore,
} from "../state/release-check-store";
import { useI18n } from "../i18n/I18nContext";
import type { MessageKey } from "../i18n/catalogues";

const REPO_URL = "https://github.com/nao7sep/onecopy";

export default function AboutModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useI18n();
  // Keys, not finished sentences, so the messages follow a language change.
  const [linkFailures, setLinkFailures] = useState<Partial<Record<"repository" | "issues" | "release", MessageKey>>>({});
  const linkAttempts = useRef({ repository: 0, issues: 0, release: 0 });
  const checkingRelease = useReleaseCheckStore((state) => state.checking);
  const releaseResult = useReleaseCheckStore((state) => state.manualResult);
  if (!open) return null;

  const openProjectPage = async (owner: "repository" | "issues" | "release", url: string, failure: MessageKey) => {
    const attempt = ++linkAttempts.current[owner];
    try {
      await openUrl(url);
      if (linkAttempts.current[owner] !== attempt) return;
      setLinkFailures((current) => {
        const next = { ...current };
        delete next[owner];
        return next;
      });
    } catch (error) {
      log.warn("about link open failed", { url, ...toErrorFields(error) });
      recordActionFailure("about-link-open-failed", message(failure), error);
      if (linkAttempts.current[owner] !== attempt) return;
      setLinkFailures((current) => ({ ...current, [owner]: failure }));
    }
  };

  return (
    <ModalShell title={t("about.title")} onClose={onClose} widthClass="w-[400px]">
      {/* Left-aligned like every other surface in the app. Centering a block
          of prose and two buttons only reads as deliberate when it is a splash
          screen; here it made the modal look unfinished. */}
      <div className="flex flex-col gap-1">
        <p className="text-base font-semibold text-ink-strong">OneCopy</p>
        <p className="text-xs text-ink-muted">{t("about.version", { version: __APP_VERSION__ })}</p>
        <p className="mt-2 text-sm text-ink">
          {t("about.description")}
        </p>
        <div className="mt-4 flex gap-2">
          <Button onClick={() => void openProjectPage("repository", REPO_URL, "about.githubOpenFailed")}>GitHub</Button>
          <Button onClick={() => void openProjectPage("issues", `${REPO_URL}/issues`, "about.issuesOpenFailed")}>{t("about.reportIssue")}</Button>
        </div>
        <div className="mt-3">
          <Button
            disabled={checkingRelease}
            onClick={() => void useReleaseCheckStore.getState().checkManual()}
          >
            {checkingRelease ? t("about.checking") : t("about.checkRelease")}
          </Button>
        </div>
        {releaseResult !== null ? (
          <OperationResult
            level={releaseResult.status === "failed" ? "error" : "info"}
            className="mt-3"
            actions={releaseResult.status === "newer" ? (
              <Button onClick={() => void openProjectPage("release", LATEST_RELEASE_PAGE, "about.releaseOpenFailed")}>
                {t("about.viewRelease")}
              </Button>
            ) : undefined}
          >
            {releaseResult.status === "newer"
              ? t("about.newerAvailable", { version: releaseResult.version })
              : releaseResult.status === "current"
                ? t("about.current")
                : t("about.checkFailed")}
          </OperationResult>
        ) : null}
        {linkFailures.repository ? (
          <OperationResult
            level="error"
            className="mt-3"
            onDismiss={() => setLinkFailures((current) => {
              const next = { ...current };
              delete next.repository;
              return next;
            })}
            dismissLabel={t("about.closeGithubResult")}
          >
            {t(linkFailures.repository)}
          </OperationResult>
        ) : null}
        {linkFailures.issues ? (
          <OperationResult
            level="error"
            className="mt-3"
            onDismiss={() => setLinkFailures((current) => {
              const next = { ...current };
              delete next.issues;
              return next;
            })}
            dismissLabel={t("about.closeIssuesResult")}
          >
            {t(linkFailures.issues)}
          </OperationResult>
        ) : null}
        {linkFailures.release ? (
          <OperationResult
            level="error"
            className="mt-3"
            onDismiss={() => setLinkFailures((current) => {
              const next = { ...current };
              delete next.release;
              return next;
            })}
            dismissLabel={t("about.closeReleaseResult")}
          >
            {t(linkFailures.release)}
          </OperationResult>
        ) : null}
        <p className="mt-5 text-xs text-ink-muted">{t("about.copyright")}</p>
      </div>
    </ModalShell>
  );
}
