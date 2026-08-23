// The About surface (modal-dialog conventions' required payload): name,
// version, one-line description, repository/issues links, copyright, license.

import { openUrl } from "@tauri-apps/plugin-opener";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";

const REPO_URL = "https://github.com/nao7sep/onecopy";

export default function AboutModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  if (!open) return null;
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
          <Button onClick={() => void openUrl(REPO_URL)}>GitHub</Button>
          <Button onClick={() => void openUrl(`${REPO_URL}/issues`)}>Report an issue</Button>
        </div>
        <p className="mt-5 text-xs text-ink-muted">© 2026 Yoshinao Inoguchi · MIT License</p>
      </div>
    </ModalShell>
  );
}
