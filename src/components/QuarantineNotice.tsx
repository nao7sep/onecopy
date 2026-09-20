import { useAppStore } from "../state/app-store";
import type { QuarantineRecord } from "../repositories";
import { useI18n } from "../i18n/I18nContext";
import type { Translator } from "../i18n/translate";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";

// What the user is told when a settings file would not parse.
//
// The core sets the unreadable file aside rather than resetting over it, which
// preserves whatever was in there — but a set-aside nobody mentions is just a
// silent reset with extra steps (storage-path-conventions). So this surface is
// half of that recovery, not a courtesy: it names the file it could not read,
// says that the original bytes were preserved and locatable through the log,
// explains what the app is running on instead, and says what it did NOT touch.
// Dismissible, because there is nothing to decide — the recovery already happened.

/** What starting over means for each store, in the user's terms. Falls back to
 * a neutral phrasing so an unlisted store still reports honestly. */
function startedWith(t: Translator["t"], file: string): string {
  switch (file) {
    case "config.json":
      return t("quarantine.startedWithConfig");
    case "state.json":
      return t("quarantine.startedWithState");
    default:
      return t("quarantine.startedWithDefaults");
  }
}

function Record({ record }: { record: QuarantineRecord }) {
  const { t, rich } = useI18n();
  return (
    <li className="rounded-lg border border-border p-3">
      <p className="text-sm text-ink-strong">
        {rich("quarantine.fileUnreadable", {
          file: <span className="font-semibold">{record.file}</span>,
        })}
      </p>
      <p className="mt-1 text-sm text-ink">{startedWith(t, record.file)}</p>
      <p className="mt-2 text-xs text-ink-muted">
        {t("quarantine.originalPreserved")}
      </p>
    </li>
  );
}

export default function QuarantineNotice() {
  const quarantines = useAppStore((s) => s.quarantines);
  const dismiss = useAppStore((s) => s.dismissQuarantines);
  const { t } = useI18n();

  if (quarantines.length === 0) return null;

  return (
    <ModalShell
      title={t("quarantine.title")}
      onClose={dismiss}
      widthClass="w-[min(820px,calc(100vw-3rem))]"
    >
      <ul className="space-y-2">
        {quarantines.map((record) => (
          <Record key={record.quarantinedTo} record={record} />
        ))}
      </ul>
      <p className="mt-3 text-sm text-ink-muted">{t("quarantine.nothingElse")}</p>
      <div className="mt-4 flex justify-end">
        <Button variant="primary" onClick={dismiss}>
          {t("quarantine.ok")}
        </Button>
      </div>
    </ModalShell>
  );
}
