import { useBinariesStore } from "../state/binaries-store";
import { useAppStore } from "../state/app-store";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import { Row, Toggle } from "./ui/Field";
import { formatLocalMinute } from "../utils/displayTime";

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
  const checkAtLaunch =
    useAppStore((s) => s.appData?.config?.checkUpdatesAtLaunch) === true;

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
      <div className="rounded-xl border border-border p-3 text-sm">
        <div className="flex items-center justify-between">
          <span className="font-semibold text-ink-strong">ffmpeg</span>
          <span className="text-xs text-ink-muted">
            {state ? STATUS_LABELS[state.status] : "…"}
          </span>
        </div>
        {/* Ordered the way the facts are READ: when it was last checked
            decides how much the next two are worth, and the latest known
            version is the thing the installed one is being judged against.
            Installed last, as the conclusion. */}
        <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
          <dt className="text-ink-muted">Checked</dt>
          <dd className="text-ink">
            {state?.facts.lastCheckedAtUtc
              ? formatLocalMinute(state.facts.lastCheckedAtUtc)
              : "Never"}
          </dd>
          <dt className="text-ink-muted">Latest known</dt>
          <dd className="text-ink">{state?.facts.latestKnownVersion ?? "—"}</dd>
          <dt className="text-ink-muted">Installed</dt>
          <dd className="text-ink">{state?.facts.installedVersion ?? "—"}</dd>
        </dl>
        {installing ? (
          <p className="mt-3 text-xs text-primary">{progress}</p>
        ) : (
          <div className="mt-3 flex gap-2">
            {action ? (
              <Button variant="primary" onClick={() => void install()}>
                {action}
              </Button>
            ) : null}
            <Button onClick={() => void check()}>Check for updates</Button>
          </div>
        )}
      </div>
      {/* The conventions' ONE update switch, living in the management
          surface: launch-time checks for installed tools, ~daily at most.
          Default off — nothing automatic unless asked for. */}
      <div className="mt-4">
        <Row
          label="Check for updates at launch"
          hint="About once a day. Nothing is ever installed without asking."
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
        Videos and HEIC photos need ffmpeg; everything else works without it.
      </p>
    </ModalShell>
  );
}
