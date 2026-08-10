// The About surface (modal-dialog conventions' required payload): name,
// version, one-line description, repository/issues links, copyright, license.

import { openUrl } from "@tauri-apps/plugin-opener";
import ModalShell from "./ModalShell";

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
      <div className="flex flex-col items-center gap-2 py-2 text-center">
        <p className="text-lg font-semibold text-ink-strong">OneCopy</p>
        <p className="text-xs text-ink-muted">Version {__APP_VERSION__}</p>
        <p className="text-sm text-ink">
          An inbox-zero dedup handler for photos, videos, and other files.
        </p>
        <div className="mt-2 flex gap-2">
          <button
            className="rounded border border-border px-2 py-0.5 text-xs text-primary hover:bg-primary-surface"
            onClick={() => void openUrl(REPO_URL)}
          >
            GitHub
          </button>
          <button
            className="rounded border border-border px-2 py-0.5 text-xs text-primary hover:bg-primary-surface"
            onClick={() => void openUrl(`${REPO_URL}/issues`)}
          >
            Report an issue
          </button>
        </div>
        <p className="mt-3 text-xs text-ink-muted">
          © 2026 Yoshinao Inoguchi · MIT License
        </p>
      </div>
    </ModalShell>
  );
}
