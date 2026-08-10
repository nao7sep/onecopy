import { useBinariesStore } from "../state/binaries-store";
import ModalShell from "./ModalShell";

// The "Managed tools" modal: one row per tool (ffmpeg today), the one
// context-aware action, an explicit check button, a labelled Close. It never
// checks on open — the button is the only trigger.

const STATUS_LABELS: Record<string, string> = {
  "not-installed": "Not installed",
  "update-available": "Update available",
  "up-to-date": "Up to date",
  "installed-unchecked": "Installed (not checked)",
};

export default function BinariesModal() {
  const open = useBinariesStore((s) => s.modalOpen);
  const state = useBinariesStore((s) => s.state);
  const installing = useBinariesStore((s) => s.installing);
  const progress = useBinariesStore((s) => s.progress);
  const install = useBinariesStore((s) => s.install);
  const check = useBinariesStore((s) => s.check);
  const setModalOpen = useBinariesStore((s) => s.setModalOpen);

  if (!open) return null;

  const action =
    state === null
      ? null
      : state.status === "not-installed"
        ? "Install"
        : state.status === "update-available"
          ? "Update"
          : null;

  return (
    <ModalShell title="Managed tools" onClose={() => setModalOpen(false)}>
      <div className="rounded border border-border p-2 text-sm">
        <div className="flex items-center justify-between">
          <span className="font-medium text-ink-strong">ffmpeg</span>
          <span className="text-xs text-ink-muted">
            {state ? STATUS_LABELS[state.status] : "…"}
          </span>
        </div>
        <div className="mt-1 text-xs text-ink-muted">
          Installed: {state?.facts.installedVersion ?? "—"} · Latest known:{" "}
          {state?.facts.latestKnownVersion ?? "—"}
          {state?.facts.lastCheckedAtUtc
            ? ` · Checked ${state.facts.lastCheckedAtUtc}`
            : " · Never checked"}
        </div>
        {installing ? (
          <p className="mt-2 text-xs text-primary">{progress}</p>
        ) : (
          <div className="mt-2 flex gap-2">
            {action ? (
              <button
                className="rounded bg-primary px-2 py-0.5 text-xs text-ink-inverted"
                onClick={() => void install()}
              >
                {action}
              </button>
            ) : null}
            <button
              className="rounded border border-border px-2 py-0.5 text-xs text-ink hover:bg-surface-muted"
              onClick={() => void check()}
            >
              Check for updates
            </button>
          </div>
        )}
      </div>
      <p className="mt-2 text-xs text-ink-muted">
        Video posters and snapshot strips need ffmpeg; photos work without it.
      </p>
    </ModalShell>
  );
}
