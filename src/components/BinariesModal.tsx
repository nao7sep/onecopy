import {
  useBinariesStore,
  type DependencyState,
  type InstallStep,
} from "../state/binaries-store";
import { managedInstallActivityLine } from "../models/dependencyProgress";
import { useAppStore } from "../state/app-store";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import { Row, Toggle } from "./ui/Field";
import { formatLocalMinute } from "../utils/displayTime";

const NO_INSTALL_HISTORY: InstallStep[] = [];

// "Managed tools" — grouped by the two genuinely different LIFECYCLES the
// registry holds (developer, 2026-08-17; one flat list forced an update
// vocabulary that only a single entry could honor):
//
//   ffmpeg resolves live from upstream. It has a real version, a real
//   "latest", and a check worth running — so the check button lives in ITS
//   row, where its scope is unmistakable. A registry-wide "Check for
//   updates" that in truth only ever checked ffmpeg was a promise the app
//   could not keep.
//
//   The models are chosen BY THIS APP BUILD. There is no upstream to ask, so
//   they never say "Up to date" (a claim about a comparison nobody made) —
//   installed is simply "Installed" — and they never carry a "checked at"
//   stamp. They do show their upstream RELEASE DATE, which is the only
//   honest answer to "how old is this model?". A re-pinned model still
//   surfaces as an update, because it genuinely is one: this app version now
//   expects a different file.
//
// Installs run in PARALLEL, so one row's download never disables another's.

/** What a row's state is called, which depends on whether "latest" is a
 * thing this entry can even have. */
function statusLabel(entry: DependencyState): string {
  if (entry.status === "not-installed") return "Not installed";
  if (entry.status === "update-available") return "Update available";
  // Only a live-resolved entry may claim up-to-date: it was actually
  // compared against its upstream. A model has nothing to compare with.
  if (!entry.checkable) return "Installed";
  return entry.status === "up-to-date" ? "Up to date" : "Installed";
}

function displayArtifactIdentity(identity: string): string {
  return identity.match(/^Latest Auto-Build \((.+)\)$/)?.[1] ?? identity;
}

/** The one line of version fact a row shows. A present entry whose version could
 * not be read says so — silence would leave an "Installed" row with an Update
 * button and no explanation of why it is offered. */
function factLine(entry: DependencyState): string | null {
  const released = entry.released !== null ? `Released ${entry.released}` : null;
  if (entry.status === "not-installed") return released;
  if (!entry.checkable) return released;
  const installed = entry.installedVersion;
  const latest = entry.facts.latestKnownVersion;
  const version =
    entry.status === "update-available" && installed !== null && latest !== null
      ? `Build ${displayArtifactIdentity(installed)} · ${displayArtifactIdentity(latest)} available`
      : installed !== null
        ? `Build ${displayArtifactIdentity(installed)}`
        : "Version unreadable";
  return [version, released].filter((part) => part !== null).join(" · ") || null;
}

function EntryRow({ entry }: { entry: DependencyState }) {
  const progress = useBinariesStore((s) => s.installing[entry.id]);
  const history = useBinariesStore(
    (s) => s.installHistory[entry.id] ?? NO_INSTALL_HISTORY,
  );
  const error = useBinariesStore((s) => s.errors[entry.id]);
  const checking = useBinariesStore((s) => s.checking);
  const checkingId = useBinariesStore((s) => s.checkingId);
  const checkCancelling = useBinariesStore((s) => s.checkCancelling);
  const cooldownUntil = useBinariesStore((s) => s.cooldownUntil);
  const lastCheckOutcome = useBinariesStore((s) => s.lastCheckOutcome);
  const install = useBinariesStore((s) => s.install);
  const cancel = useBinariesStore((s) => s.cancel);
  const checkAll = useBinariesStore((s) => s.checkAll);
  const cancelCheck = useBinariesStore((s) => s.cancelCheck);
  const installing = progress !== undefined;
  const progressLine = progress === undefined ? null : managedInstallActivityLine(progress);
  const visibleHistory =
    history.length > 0 || progress === undefined
      ? history
      : [{ phase: "active", text: progressLine ?? "Starting…" }];

  // Install when missing, Update when a newer version is known — and Update
  // again when a present entry's own version could not be read, which is the
  // only way out of that row: a check resolves the LATEST, so it can never
  // clear an unreadable INSTALLED version, and re-acquiring is what replaces
  // the copy that would not answer.
  const action =
    entry.status === "not-installed"
      ? "Install"
      : entry.status === "update-available" ||
          (entry.status === "installed-unchecked" && entry.installedVersion === null)
        ? "Update"
        : null;
  const fact = factLine(entry);
  // Only a checkable, installed entry offers a check — and ffmpeg is the only
  // checkable entry, so this IS the single check button, standing where its
  // scope is obvious rather than floating above a list it cannot cover.
  const offersCheck = entry.checkable && entry.status !== "not-installed";
  const coolingDown = !checking && Date.now() < cooldownUntil;
  const checked =
    entry.facts.lastCheckedAtUtc !== null
      ? `Checked ${formatLocalMinute(entry.facts.lastCheckedAtUtc)}`
      : null;

  return (
    <div className="rounded-xl border border-border p-3 text-sm">
      <div className="flex items-center justify-between gap-3">
        <span className="min-w-0 truncate font-semibold text-ink-strong">{entry.label}</span>
        <span className="shrink-0 text-xs text-ink-muted">{statusLabel(entry)}</span>
      </div>
      {fact !== null ? <p className="mt-1 text-xs text-ink-muted">{fact}</p> : null}
      {visibleHistory.length > 0 ? (
        <ol
          className="mt-2 space-y-0.5"
          aria-label={`${entry.label} install progress`}
        >
          {visibleHistory.map((step, index) => (
            <li
              key={step.phase}
              className={`text-xs ${
                installing && index === visibleHistory.length - 1
                  ? "text-primary"
                  : step.phase === "result"
                    ? "font-medium text-ink"
                    : "text-ink-muted"
              }`}
            >
              {step.text}
            </li>
          ))}
        </ol>
      ) : null}
      {error !== undefined && !installing ? (
        <p className="mt-1 text-xs text-danger">{error}</p>
      ) : null}
      {installing ? (
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <Button
            disabled={progress?.cancelling === true}
            onClick={() => void cancel(entry.id)}
          >
            {progress?.cancelling === true ? "Cancelling…" : "Cancel"}
          </Button>
        </div>
      ) : action !== null || offersCheck ? (
        <div className="mt-2 flex flex-wrap items-center gap-2">
          {action !== null ? (
            <Button variant="primary" onClick={() => void install(entry.id)}>
              {action}
            </Button>
          ) : null}
          {offersCheck ? (
            <>
              {checkingId === entry.id ? (
                <>
                  <span className="text-xs text-primary">
                    {checkCancelling ? "Cancelling…" : "Checking…"}
                  </span>
                  <Button
                    disabled={checkCancelling}
                    onClick={() => void cancelCheck(entry.id)}
                  >
                    Cancel check
                  </Button>
                </>
              ) : (
                <Button disabled={checking || coolingDown} onClick={() => void checkAll()}>
                  Check for updates
                </Button>
              )}
              {lastCheckOutcome !== null ? (
                <span className="text-xs font-medium text-ink">{lastCheckOutcome}</span>
              ) : checked !== null ? (
                <span className="text-xs text-ink-muted">{checked}</span>
              ) : null}
            </>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

export default function BinariesModal() {
  const open = useBinariesStore((s) => s.modalOpen);
  const entries = useBinariesStore((s) => s.entries);
  const loading = useBinariesStore((s) => s.loading);
  const loadError = useBinariesStore((s) => s.loadError);
  const installing = useBinariesStore((s) => s.installing);
  const setModalOpen = useBinariesStore((s) => s.setModalOpen);
  const installAll = useBinariesStore((s) => s.installAll);
  const checkAtLaunch =
    useAppStore((s) => s.appData?.config?.checkUpdatesAtLaunch) === true;

  if (!open) return null;

  const actionable = entries.filter(
    (entry) =>
      (entry.status === "not-installed" || entry.status === "update-available") &&
      installing[entry.id] === undefined,
  ).length;
  const upstream = entries.filter((entry) => entry.checkable);
  const appSelected = entries.filter((entry) => !entry.checkable);

  return (
    <ModalShell
      title="Managed tools"
      onClose={() => setModalOpen(false)}
      footerStart={entries.length > 0 ? loadError : undefined}
    >
      {actionable > 1 ? (
        <div className="mb-3">
          <Button variant="primary" onClick={() => void installAll()}>
            Install all
          </Button>
        </div>
      ) : null}

      {entries.length === 0 ? (
        <p className={`py-4 text-center text-sm ${loadError !== null ? "text-danger" : "text-ink-muted"}`}>
          {loading
            ? "Loading managed tools…"
            : loadError ?? "No managed tools are configured."}
        </p>
      ) : null}

      <div className="space-y-2">
        {upstream.map((entry) => (
          <EntryRow key={entry.id} entry={entry} />
        ))}
      </div>

      {appSelected.length > 0 ? (
        <section className="mt-5">
          <h3 className="text-sm font-semibold text-ink-strong">Models selected by OneCopy</h3>
          <p className="mb-2 text-xs text-ink-muted">
            These models are downloaded only when you install them here.
            OneCopy selects the versions, so they change only when the app
            updates — there is nothing to check for.
          </p>
          <div className="space-y-2">
            {appSelected.map((entry) => (
              <EntryRow key={entry.id} entry={entry} />
            ))}
          </div>
        </section>
      ) : null}

      {/* The conventions' ONE update switch, living in the management
          surface. It covers the tools that HAVE upstream updates — ffmpeg
          today — at most about once a day. Default off. */}
      <div className="mt-5">
        <Row
          label="Check for ffmpeg updates at launch"
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
        Videos and HEIC photos need ffmpeg; the models power transcription and
        smarter similar-photo matching. Everything else works without them.
      </p>
    </ModalShell>
  );
}
