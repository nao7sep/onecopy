import { useBinariesStore, type DependencyState } from "../state/binaries-store";
import { useAppStore } from "../state/app-store";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import { Row, Toggle } from "./ui/Field";
import { formatLocalMinute } from "../utils/displayTime";

// The "Managed tools" modal: ONE ROW PER REGISTRY ENTRY — the ffmpeg binary
// and the model files — each with the one context-aware action and an
// explicit check button. It never checks on open — the buttons and the
// launch toggle are the only triggers.

const STATUS_LABELS: Record<string, string> = {
  "not-installed": "Not installed",
  "update-available": "Update available",
  "up-to-date": "Up to date",
  "installed-unchecked": "Installed (not checked)",
};

function EntryRow({ entry }: { entry: DependencyState }) {
  const installingId = useBinariesStore((s) => s.installingId);
  const progress = useBinariesStore((s) => s.progress);
  const install = useBinariesStore((s) => s.install);
  const check = useBinariesStore((s) => s.check);
  const installing = installingId === entry.id;
  const busyElsewhere = installingId !== null && !installing;

  const action =
    entry.status === "not-installed"
      ? "Install"
      : entry.status === "update-available"
        ? "Update"
        : null;

  return (
    <div className="rounded-xl border border-border p-3 text-sm">
      <div className="flex items-center justify-between gap-3">
        <span className="min-w-0 truncate font-semibold text-ink-strong">{entry.label}</span>
        <span className="shrink-0 text-xs text-ink-muted">
          {STATUS_LABELS[entry.status] ?? entry.status}
        </span>
      </div>
      {/* Ordered the way the facts are READ: when it was last checked decides
          how much the next two are worth, and the latest known version is the
          thing the installed one is being judged against. */}
      <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
        <dt className="text-ink-muted">Checked</dt>
        <dd className="text-ink">
          {entry.facts.lastCheckedAtUtc
            ? formatLocalMinute(entry.facts.lastCheckedAtUtc)
            : "Never"}
        </dd>
        <dt className="text-ink-muted">Latest known</dt>
        <dd className="text-ink">{entry.facts.latestKnownVersion ?? "—"}</dd>
        <dt className="text-ink-muted">Installed</dt>
        <dd className="text-ink">{entry.facts.installedVersion ?? "—"}</dd>
      </dl>
      {installing ? (
        <p className="mt-3 text-xs text-primary">{progress}</p>
      ) : (
        <div className="mt-3 flex gap-2">
          {action ? (
            <Button
              variant="primary"
              disabled={busyElsewhere}
              onClick={() => void install(entry.id)}
            >
              {action}
            </Button>
          ) : null}
          <Button disabled={busyElsewhere} onClick={() => void check(entry.id)}>
            Check for updates
          </Button>
        </div>
      )}
    </div>
  );
}

export default function BinariesModal() {
  const open = useBinariesStore((s) => s.modalOpen);
  const entries = useBinariesStore((s) => s.entries);
  const setModalOpen = useBinariesStore((s) => s.setModalOpen);
  const checkAtLaunch =
    useAppStore((s) => s.appData?.config?.checkUpdatesAtLaunch) === true;

  if (!open) return null;

  return (
    <ModalShell title="Managed tools" onClose={() => setModalOpen(false)}>
      <div className="space-y-2">
        {entries.length === 0 ? (
          <p className="py-4 text-center text-sm text-ink-muted">…</p>
        ) : (
          entries.map((entry) => <EntryRow key={entry.id} entry={entry} />)
        )}
      </div>
      {/* The conventions' ONE update switch, living in the management
          surface: launch-time checks for INSTALLED entries, ~daily at most.
          Default off — nothing automatic unless asked for. */}
      <div className="mt-4">
        <Row
          label="Check for updates at launch"
          hint="About once a day, installed tools only. Nothing is ever installed without asking."
        >
          <Toggle
            checked={checkAtLaunch}
            onChange={(checked) =>
              void useAppStore.getState().patchConfig({ checkUpdatesAtLaunch: checked })
            }
          />
        </Row>
      </div>
      <p className="mt-2 text-xs text-ink-muted">
        Videos and HEIC photos need ffmpeg; the models power transcription and
        smarter similar-photo matching. Everything else works without them.
      </p>
    </ModalShell>
  );
}
