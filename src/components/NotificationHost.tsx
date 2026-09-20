import { useEffect, useRef } from "react";
import { conditionText } from "../models/noticeConditions";
import { noticeRaisedHere } from "../state/notifications-store";
import { X } from "lucide-react";
import { useI18n } from "../i18n/I18nContext";
import { message, type Message, type Translator } from "../i18n/translate";
import { useAppStore } from "../state/app-store";
import {
  installNotificationWiring,
  type NotificationRecord,
  useNotificationsStore,
} from "../state/notifications-store";
import {
  presentEscapedFailure,
  recordInterfaceFailure,
} from "../utils/failureSurface";
import { log, toErrorFields } from "../repositories";
import { useReleaseCheckStore } from "../state/release-check-store";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";

function ReleaseNotice() {
  const { t, text } = useI18n();
  const version = useReleaseCheckStore((state) => state.noticeVersion);
  const linkError = useReleaseCheckStore((state) => state.noticeLinkError);
  const dismiss = useReleaseCheckStore((state) => state.dismissNotice);
  const openRelease = useReleaseCheckStore((state) => state.openNoticeRelease);
  if (version === null) return null;
  return (
    <section
      data-release-notice
      role="status"
      className="pointer-events-auto w-full rounded-lg border border-border bg-surface p-3 text-ink"
    >
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <p className="select-text break-words text-sm">
            {t("about.newerAvailable", { version })}
          </p>
          <Button className="mt-2" onClick={() => void openRelease()}>
            {t("about.viewRelease")}
          </Button>
        </div>
        <button
          aria-label={t("notice.dismissRelease")}
          title={t("common.dismiss")}
          className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-current opacity-70 hover:bg-ink/10 hover:opacity-100 focus-visible:bg-ink/10 focus-visible:opacity-100"
          onClick={dismiss}
        >
          <X size={14} />
        </button>
      </div>
      {linkError !== null ? (
        <OperationResult level="error" className="mt-2">
          {text(linkError)}
        </OperationResult>
      ) : null}
    </section>
  );
}

function Toast({
  record,
  durationMs,
}: {
  record: NotificationRecord;
  durationMs: number;
}) {
  const { t, text } = useI18n();
  const dismiss = useNotificationsStore((state) => state.dismiss);
  const dismissing = useNotificationsStore((state) => state.dismissing.has(record.id));
  const remaining = useRef(durationMs);
  const started = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const stopTimer = () => {
    if (timer.current === null) return;
    clearTimeout(timer.current);
    timer.current = null;
    remaining.current = Math.max(0, remaining.current - (performance.now() - started.current));
  };
  const startTimer = () => {
    if (
      record.presentation !== "timed" ||
      timer.current !== null ||
      remaining.current <= 0
    ) {
      return;
    }
    started.current = performance.now();
    timer.current = setTimeout(() => {
      timer.current = null;
      remaining.current = 0;
      void dismiss(record.id);
    }, remaining.current);
  };

  useEffect(() => {
    remaining.current = durationMs;
    startTimer();
    return stopTimer;
    // A repeated coalesced notice resets its one visible timer.
  }, [durationMs, record.id, record.lastSeenUtc]);

  const tone =
    record.level === "error"
      ? "border-danger/60 bg-danger-surface text-danger"
      : record.level === "warning"
        ? "border-warning/60 bg-warning-surface text-ink"
        : "border-border bg-surface text-ink";

  return (
    <section
      data-notification
      role={record.level === "error" ? "alert" : "status"}
      className={`pointer-events-auto w-full rounded-lg border p-3 ${tone}`}
      onMouseEnter={stopTimer}
      onMouseLeave={startTimer}
      onFocusCapture={stopTimer}
      onBlurCapture={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) startTimer();
      }}
    >
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          {/* This window's own notice keeps its descriptor and follows a
              language change; one the core raised is said from its condition,
              and a path is the user's own. */}
          <p className="select-text break-words text-sm">
            {noticeWords(record, text, t)}
          </p>
          {record.path ? (
            <p className="mt-1 select-text break-all text-xs opacity-70">{record.path}</p>
          ) : null}
        </div>
        <button
          aria-label={t("notice.dismissNotification")}
          title={t("common.dismiss")}
          disabled={dismissing}
          className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-current opacity-70 hover:bg-ink/10 hover:opacity-100 focus-visible:bg-ink/10 focus-visible:opacity-100 disabled:opacity-30"
          onClick={() => void dismiss(record.id)}
        >
          <X size={14} />
        </button>
      </div>
      {record.occurrenceCount > 1 ? (
        <p className="mt-1 text-xs opacity-70">
          {t("notice.occurrences", { count: record.occurrenceCount })}
        </p>
      ) : null}
    </section>
  );
}

function noticeWords(
  record: NotificationRecord,
  text: (message: Message) => string,
  t: Translator["t"],
): string {
  const raised = noticeRaisedHere(record.id);
  return raised === undefined ? conditionText(record.kind, record.message, t) : text(raised);
}

export default function NotificationHost() {
  const { t } = useI18n();
  const active = useNotificationsStore((state) => state.active);
  const releaseVersion = useReleaseCheckStore((state) => state.noticeVersion);
  const configuredSeconds = useAppStore((state) => {
    const value = state.appData?.config?.notificationDisplaySeconds;
    return typeof value === "number" && Number.isFinite(value) ? value : 6;
  });
  const durationMs = Math.min(60, Math.max(1, configuredSeconds)) * 1000;

  useEffect(() => {
    void installNotificationWiring().catch((error) => {
      log.error("notification interface wiring failed", toErrorFields(error));
      const direct = message("notice.unavailable");
      presentEscapedFailure(direct);
      recordInterfaceFailure(direct);
    });
  }, []);

  if (active.length === 0 && releaseVersion === null) return null;
  // The active notices scroll inside a region that clips its children, which would cut
  // off each card's own shadow. The host draws the shadow from what it renders instead:
  // the shadow-xl elevation as drop-shadows, whose blur is a standard deviation (half a
  // box-shadow radius). A filter neither clips nor widens the host's hit area.
  return (
    <div
      data-notification-host
      className="pointer-events-none fixed right-4 top-4 z-[25] flex max-h-[calc(100vh-2rem)] w-[min(420px,calc(100vw-2rem))] flex-col items-stretch gap-2 [filter:drop-shadow(0_20px_12.5px_rgb(0_0_0/0.1))_drop-shadow(0_8px_5px_rgb(0_0_0/0.1))]"
    >
      <ReleaseNotice />
      {active.length > 0 ? (
        <div
          data-notification-scroll-region
          role="region"
          aria-label={t("notice.activeRegion")}
          className="pointer-events-auto min-h-0 overflow-y-auto overscroll-contain"
        >
          <div className="flex flex-col items-stretch gap-2 pr-1">
            {active.map((record) => (
              <Toast key={record.id} record={record} durationMs={durationMs} />
            ))}
          </div>
        </div>
      ) : null}
    </div>
  );
}
