// The About surface (modal-dialog conventions' required payload): name,
// version, one-line description, repository/issues links, copyright, license.

import { useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { log, toErrorFields } from "../repositories";
import { recordActionFailure } from "../state/notifications-store";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";
import {
  LATEST_RELEASE_PAGE,
  useReleaseCheckStore,
} from "../state/release-check-store";

const REPO_URL = "https://github.com/nao7sep/onecopy";

export default function AboutModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const [linkFailures, setLinkFailures] = useState<Partial<Record<"repository" | "issues" | "release", string>>>({});
  const linkAttempts = useRef({ repository: 0, issues: 0, release: 0 });
  const checkingRelease = useReleaseCheckStore((state) => state.checking);
  const releaseResult = useReleaseCheckStore((state) => state.manualResult);
  if (!open) return null;

  const openProjectPage = async (owner: "repository" | "issues" | "release", url: string, page: string) => {
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
      const message = `Couldn’t open ${page}. Try again or open it in your browser.`;
      log.warn("about link open failed", { url, ...toErrorFields(error) });
      recordActionFailure("about-link-open-failed", message, error);
      if (linkAttempts.current[owner] !== attempt) return;
      setLinkFailures((current) => ({ ...current, [owner]: message }));
    }
  };

  return (
    <ModalShell title="About OneCopy" onClose={onClose} widthClass="w-[400px]">
      {/* Left-aligned like every other surface in the app. Centering a block
          of prose and two buttons only reads as deliberate when it is a splash
          screen; here it made the modal look unfinished. */}
      <div className="flex flex-col gap-1">
        <p className="text-base font-semibold text-ink-strong">OneCopy</p>
        <p className="text-xs text-ink-muted">Version {__APP_VERSION__}</p>
        <p className="mt-2 text-sm text-ink">
          An inbox-zero dedup handler for photos, videos, and other files.
        </p>
        <div className="mt-4 flex gap-2">
          <Button onClick={() => void openProjectPage("repository", REPO_URL, "GitHub")}>GitHub</Button>
          <Button onClick={() => void openProjectPage("issues", `${REPO_URL}/issues`, "Report an issue")}>Report an issue</Button>
        </div>
        <div className="mt-3">
          <Button
            disabled={checkingRelease}
            onClick={() => void useReleaseCheckStore.getState().checkManual()}
          >
            {checkingRelease ? "Checking GitHub…" : "Check GitHub for New Release"}
          </Button>
        </div>
        {releaseResult !== null ? (
          <OperationResult
            level={releaseResult.status === "failed" ? "error" : "info"}
            className="mt-3"
            actions={releaseResult.status === "newer" ? (
              <Button onClick={() => void openProjectPage("release", LATEST_RELEASE_PAGE, "the release page")}>
                View Release on GitHub
              </Button>
            ) : undefined}
          >
            {releaseResult.status === "newer"
              ? `OneCopy ${releaseResult.version} is available.`
              : releaseResult.status === "current"
                ? "This version of OneCopy is current."
                : "OneCopy couldn’t check GitHub. Check your connection and try again."}
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
            dismissLabel="Close GitHub result"
          >
            {linkFailures.repository}
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
            dismissLabel="Close Report an issue result"
          >
            {linkFailures.issues}
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
            dismissLabel="Close release page result"
          >
            {linkFailures.release}
          </OperationResult>
        ) : null}
        <p className="mt-5 text-xs text-ink-muted">© 2026 Yoshinao Inoguchi · GNU GPL v3 or later</p>
      </div>
    </ModalShell>
  );
}
