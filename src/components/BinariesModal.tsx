import {
  useBinariesStore,
  type DependencyState,
} from "../state/binaries-store";
import { toolLabel } from "../models/coreLabels";
import { managedInstallActivityLine } from "../models/dependencyProgress";
import { useAppStore } from "../state/app-store";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import { Row, Toggle } from "./ui/Field";
import { formatLocalMinute } from "../utils/displayTime";
import { useDisplayZone } from "../hooks/useDisplayZone";
import OperationResult from "./ui/OperationResult";
import { formatBytes } from "../models/items";
import { useI18n } from "../i18n/I18nContext";
import type { Translator } from "../i18n/translate";

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
//   Pinned runtimes and models are chosen BY THIS APP BUILD. There is no upstream to ask, so
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
function statusLabel(entry: DependencyState, t: Translator["t"]): string {
  if (entry.status === "not-installed") return t("binaries.notInstalled");
  if (entry.status === "update-available") return t("binaries.updateAvailable");
  // Only a live-resolved entry may claim up-to-date: it was actually
  // compared against its upstream. A pinned artifact has nothing to compare with.
  if (!entry.checkable) return t("binaries.installed");
  return entry.status === "up-to-date" ? t("binaries.upToDate") : t("binaries.installed");
}

function displayArtifactIdentity(identity: string): string {
  return identity.match(/^Latest Auto-Build \((.+)\)$/)?.[1] ?? identity;
}

/** The one line of version fact a row shows. A present entry whose version could
 * not be read says so — silence would leave an "Installed" row with an Update
 * button and no explanation of why it is offered. */
function factLine(entry: DependencyState, t: Translator["t"]): string | null {
  const released =
    entry.released !== null ? t("binaries.released", { date: entry.released }) : null;
  if (entry.status === "not-installed") return released;
  if (!entry.checkable) return released;
  const installed = entry.installedVersion;
  const latest = entry.facts.latestKnownVersion;
  const build = (identity: string) =>
    t("binaries.build", { version: displayArtifactIdentity(identity) });
  const version =
    entry.status === "update-available" && installed !== null && latest !== null
      ? [
          build(installed),
          t("binaries.latestAvailable", { version: displayArtifactIdentity(latest) }),
        ].join(" · ")
      : installed !== null
        ? build(installed)
        : t("binaries.versionUnreadable");
  return [version, released].filter((part) => part !== null).join(" · ") || null;
}

function EntryRow({ entry }: { entry: DependencyState }) {
  useDisplayZone();
  const { t, text, dateTime, number, percent } = useI18n();
  const progress = useBinariesStore((s) => s.installing[entry.id]);
  const error = useBinariesStore((s) => s.errors[entry.id]);
  const checking = useBinariesStore((s) => s.checking);
  const checkingId = useBinariesStore((s) => s.checkingId);
  const checkCancelling = useBinariesStore((s) => s.checkCancelling);
  const checkFeedback = useBinariesStore((s) => s.checkFeedback);
  const checkError = useBinariesStore((s) => s.checkError);
  const install = useBinariesStore((s) => s.install);
  const cancel = useBinariesStore((s) => s.cancel);
  const checkAll = useBinariesStore((s) => s.checkAll);
  const cancelCheck = useBinariesStore((s) => s.cancelCheck);
  const installing = progress !== undefined;
  const progressLine = progress === undefined ? null : text(managedInstallActivityLine(progress, number, percent));

  // Install when missing, Update when a newer version is known — and Update
  // again when a present entry's own version could not be read, which is the
  // only way out of that row: a check resolves the LATEST, so it can never
  // clear an unreadable INSTALLED version, and re-acquiring is what replaces
  // the copy that would not answer.
  const action =
    entry.status === "not-installed"
      ? "binaries.install"
      : entry.status === "update-available" ||
          (entry.status === "installed-unchecked" && entry.installedVersion === null)
        ? "binaries.update"
        : null;
  const fact = factLine(entry, t);
  // Only a checkable, installed entry offers a check — and ffmpeg is the only
  // checkable entry, so this IS the single check button, standing where its
  // scope is obvious rather than floating above a list it cannot cover.
  const offersCheck = entry.checkable && entry.status !== "not-installed";
  const checked =
    entry.facts.lastCheckedAtUtc !== null
      ? t("binaries.lastChecked", { time: formatLocalMinute(entry.facts.lastCheckedAtUtc, dateTime) })
      : null;
  const missingCoreTool =
    entry.status === "not-installed" && entry.requiredForCore;

  return (
    <div className="rounded-xl border border-border p-3 text-sm">
      <div className="flex items-center justify-between gap-3">
        <span className="min-w-0 break-words font-semibold leading-snug text-ink-strong">
          {toolLabel(entry.id, entry.label, t)}
        </span>
        <span
          className={`shrink-0 text-xs ${
            missingCoreTool ? "font-semibold text-warning" : "text-ink-muted"
          }`}
        >
          {statusLabel(entry, t)}
        </span>
      </div>
      {fact !== null || entry.downloadBytes !== null ? (
        <p className="mt-1 break-words text-xs text-ink-muted">
          {[
            fact,
            entry.downloadBytes !== null
              ? t("binaries.downloadSize", { size: formatBytes(entry.downloadBytes, number) })
              : null,
          ].filter((part) => part !== null).join(" · ")}
        </p>
      ) : null}
      {progressLine !== null ? (
        <p
          className="mt-2 text-xs text-primary"
          aria-live="polite"
          aria-atomic="true"
          aria-label={t("binaries.installProgress", { name: entry.label })}
        >
          {progressLine}
        </p>
      ) : null}
      {error !== undefined ? (
        <OperationResult level="error" className="mt-2">
          {text(error)}
        </OperationResult>
      ) : null}
      {installing ? (
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <Button
            disabled={progress?.cancelling === true}
            onClick={() => void cancel(entry.id)}
          >
            {progress?.cancelling === true ? t("common.cancelling") : t("common.cancel")}
          </Button>
        </div>
      ) : action !== null || offersCheck ? (
        <div className="mt-2 flex flex-wrap items-center gap-2">
          {action !== null ? (
            <Button variant="primary" onClick={() => void install(entry.id)}>
              {t(action)}
            </Button>
          ) : null}
          {offersCheck ? (
            <>
              <Button disabled={checking} onClick={() => void checkAll()}>
                {checking
                  ? checkCancelling
                    ? t("common.cancelling")
                    : t("binaries.checking")
                  : checkFeedback === "checked"
                    ? t("binaries.checkDone")
                    : t("binaries.checkForUpdates")}
              </Button>
              {checkingId === entry.id ? (
                  <Button
                    disabled={checkCancelling}
                    onClick={() => void cancelCheck(entry.id)}
                  >
                    {t("binaries.cancelCheck")}
                  </Button>
              ) : checked !== null ? (
                <span className="text-xs text-ink-muted">{checked}</span>
              ) : null}
            </>
          ) : null}
        </div>
      ) : null}
      {offersCheck && checkError !== null ? (
        <OperationResult level="error" className="mt-2">
          {text(checkError)}
        </OperationResult>
      ) : null}
    </div>
  );
}

export default function BinariesModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const { t, text } = useI18n();
  const entries = useBinariesStore((s) => s.entries);
  const loading = useBinariesStore((s) => s.loading);
  const loadError = useBinariesStore((s) => s.loadError);
  const installing = useBinariesStore((s) => s.installing);
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
      title={t("binaries.title")}
      onClose={onClose}
      widthClass="w-[min(680px,calc(100vw-3rem))]"
      footerResult={
        entries.length > 0 && loadError !== null ? (
          <OperationResult level="error">{text(loadError)}</OperationResult>
        ) : undefined
      }
    >
      {actionable > 1 ? (
        <div className="mb-3">
          <Button variant="primary" onClick={() => void installAll()}>
            {t("binaries.installAll")}
          </Button>
        </div>
      ) : null}

      {entries.length === 0 ? (
        loadError !== null ? (
          <OperationResult level="error" className="my-4">
            {text(loadError)}
          </OperationResult>
        ) : (
          <p className="py-4 text-center text-sm text-ink-muted">
            {loading ? t("binaries.loading") : t("binaries.none")}
          </p>
        )
      ) : null}

      <div className="space-y-2">
        {upstream.map((entry) => (
          <EntryRow key={entry.id} entry={entry} />
        ))}
      </div>

      {appSelected.length > 0 ? (
        <section className="mt-5">
          <h3 className="text-sm font-semibold text-ink-strong">{t("binaries.appSelected")}</h3>
          <p className="mb-2 text-xs text-ink-muted">
            {t("binaries.appSelectedNote")}
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
          label={t("binaries.checkAtLaunch")}
          hint={t("binaries.checkAtLaunchHint")}
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
        {t("binaries.toolsNote")}
      </p>
    </ModalShell>
  );
}
