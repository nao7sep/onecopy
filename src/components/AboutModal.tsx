// The About surface (modal-dialog conventions' required payload): name,
// version, one-line description, repository/issues links, copyright, license.

import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { log, toErrorFields } from "../repositories";
import { recordActionFailure } from "../state/notifications-store";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";

const REPO_URL = "https://github.com/nao7sep/onecopy";

export default function AboutModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const [linkFailure, setLinkFailure] = useState<string | null>(null);
  if (!open) return null;

  const openProjectPage = async (url: string, page: string) => {
    setLinkFailure(null);
    try {
      await openUrl(url);
    } catch (error) {
      const message = `Couldn’t open ${page}. Try again or open it in your browser.`;
      log.warn("about link open failed", { url, ...toErrorFields(error) });
      setLinkFailure(message);
      recordActionFailure("about-link-open-failed", message, error);
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
          <Button onClick={() => void openProjectPage(REPO_URL, "GitHub")}>GitHub</Button>
          <Button onClick={() => void openProjectPage(`${REPO_URL}/issues`, "Report an issue")}>Report an issue</Button>
        </div>
        {linkFailure !== null ? (
          <OperationResult
            level="error"
            className="mt-3"
            onDismiss={() => setLinkFailure(null)}
            dismissLabel="Dismiss link result"
          >
            {linkFailure}
          </OperationResult>
        ) : null}
        <p className="mt-5 text-xs text-ink-muted">© 2026 Yoshinao Inoguchi · MIT License</p>
      </div>
    </ModalShell>
  );
}
