import { displayNativePath } from "../utils/nativePath";
import { useEffect, useRef } from "react";
import { X } from "lucide-react";
import { useI18n } from "../i18n/I18nContext";
import { message } from "../i18n/translate";
import { useAppStore } from "../state/app-store";
import {
  installNotificationWiring,
  noticeSentence,
  setNotificationAttention,
  type NotificationRecord,
  useNotificationsStore,
} from "../state/notifications-store";
import {
  presentEscapedFailure,
  recordInterfaceFailure,
} from "../utils/failureSurface";
import { log, toErrorFields } from "../repositories";
import { IconButton } from "./ui/Button";
import IdentifyingText from "./ui/IdentifyingText";
import { configNumber } from "../models/config";

function Toast({
  record,
  durationMs,
  ownsTimedDismissal,
}: {
  record: NotificationRecord;
  durationMs: number;
  // Main owns the countdown; every window contributes shared attention holds.
  ownsTimedDismissal: boolean;
}) {
  const { t, text } = useI18n();
  const dismiss = useNotificationsStore((state) => state.dismiss);
  const dismissing = useNotificationsStore((state) => state.dismissing.has(record.id));
  const paused = useNotificationsStore((state) => state.held.has(record.id) || state.localHeld.has(record.id));
  const attentionOwner = useRef(Symbol("notification attention"));
  const remaining = useRef(durationMs);
  const started = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Hovering AND focusing both pause the timer; it may resume only once
  // NEITHER holds it, so leaving with the mouse while focus is still inside
  // (or losing focus while the pointer is still over the card) does not
  // restart a countdown the user is still attending to.
  const hovering = useRef(false);
  const focused = useRef(false);

  const stopTimer = () => {
    if (timer.current === null) return;
    clearTimeout(timer.current);
    timer.current = null;
    remaining.current = Math.max(0, remaining.current - (performance.now() - started.current));
  };
  const startTimer = () => {
    if (
      !ownsTimedDismissal ||
      record.presentation !== "timed" ||
      timer.current !== null ||
      paused ||
      hovering.current ||
      focused.current
    ) {
      return;
    }
    started.current = performance.now();
    timer.current = setTimeout(() => {
      timer.current = null;
      remaining.current = 0;
      void dismiss(record.id, true);
    }, remaining.current);
  };

  useEffect(() => {
    remaining.current = durationMs;
    startTimer();
    return stopTimer;
    // A repeated coalesced notice resets its one visible timer.
  }, [durationMs, record.id, record.lastSeenUtc, ownsTimedDismissal]);

  useEffect(() => {
    if (paused) stopTimer();
    else startTimer();
    return stopTimer;
  }, [paused]);

  const updateAttention = () => {
    setNotificationAttention(attentionOwner.current, record.id,
      record.presentation === "timed" && (hovering.current || focused.current));
  };
  useEffect(() => {
    const release = () => {
      hovering.current = false;
      focused.current = false;
      setNotificationAttention(attentionOwner.current, record.id, false);
    };
    window.addEventListener("blur", release);
    return () => { window.removeEventListener("blur", release); release(); };
  }, [record.id]);

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
      onMouseEnter={() => { hovering.current = true; stopTimer(); updateAttention(); }}
      onMouseLeave={() => { hovering.current = false; updateAttention(); }}
      onFocusCapture={() => { focused.current = true; stopTimer(); updateAttention(); }}
      onBlurCapture={(event) => {
        if (event.currentTarget.contains(event.relatedTarget as Node | null)) return;
        focused.current = false;
        updateAttention();
      }}
    >
      <div className="flex items-start gap-3 text-sm">
        <div className="min-w-0 flex-1">
          {/* The condition's own sentence, in the current interface
              language; real detail the sentence cannot restate (a system
              error) appears as recorded, right after it. A
              notice is a compact surface, so it shows a summary of each —
              a few lines, the path on one — and Issues and Records, where
              every notice is recorded, show them whole. */}
          <p className="line-clamp-3 select-text break-words text-sm">
            {displayNativePath(noticeSentence(record, text))}
          </p>
          {record.messageKey != null && record.message.trim() !== "" ? (
            <p className="mt-1 line-clamp-2 select-text break-words text-xs opacity-70">{displayNativePath(record.message)}</p>
          ) : null}
          {record.path ? (
            <IdentifyingText value={record.path} path className="mt-1 select-text text-xs opacity-70" />
          ) : null}
        </div>
        <IconButton
          size="sm"
          tone="current"
          aria-label={t("notice.dismissNotification")}
          title={t("common.dismiss")}
          disabled={dismissing}
          className="oc-first-line-dismiss"
          onClick={() => void dismiss(record.id)}
        >
          <X size={14} />
        </IconButton>
      </div>
      {record.occurrenceCount > 1 ? (
        <p className="mt-1 text-xs opacity-70">
          {t("notice.occurrences", { count: record.occurrenceCount })}
        </p>
      ) : null}
    </section>
  );
}

export default function NotificationHost({
  ownsTimedDismissal = true,
}: {
  // False in the fullscreen view: Main, always open, is the one timer
  // owner. The fullscreen view still shows and lets the user dismiss
  // every notice; it just never races Main's own countdown.
  ownsTimedDismissal?: boolean;
} = {}) {
  const { t } = useI18n();
  const active = useNotificationsStore((state) => state.active);
  const configuredSeconds = useAppStore(
    (state) => configNumber(state.appData?.config, "notificationDisplaySeconds") ?? 1,
  );
  const durationMs = Math.min(60, Math.max(1, configuredSeconds)) * 1000;

  useEffect(() => {
    void installNotificationWiring().catch((error) => {
      log.error("notification interface wiring failed", toErrorFields(error));
      const direct = message("notice.unavailable");
      presentEscapedFailure(direct);
      recordInterfaceFailure(direct, error);
    });
  }, []);

  if (active.length === 0) return null;
  // The active notices scroll inside a region that clips its children, which would cut
  // off each card's own shadow. The host draws the shadow from what it renders instead:
  // the shadow-xl elevation as drop-shadows, whose blur is a standard deviation (half a
  // box-shadow radius). A filter neither clips nor widens the host's hit area.
  return (
    <div
      data-notification-host
      className="pointer-events-none fixed right-4 top-4 z-[25] flex max-h-[calc(100vh-2rem)] w-[min(420px,calc(100vw-2rem))] flex-col items-stretch gap-2 [filter:drop-shadow(0_20px_12.5px_rgb(0_0_0/0.1))_drop-shadow(0_8px_5px_rgb(0_0_0/0.1))]"
    >
      {active.length > 0 ? (
        <div
          data-notification-scroll-region
          role="region"
          aria-label={t("notice.activeRegion")}
          className="pointer-events-auto min-h-0 overflow-y-auto overscroll-contain"
        >
          <div className="flex flex-col items-stretch gap-2 pr-1">
            {active.map((record) => (
              <Toast
                key={record.id}
                record={record}
                durationMs={durationMs}
                ownsTimedDismissal={ownsTimedDismissal}
              />
            ))}
          </div>
        </div>
      ) : null}
    </div>
  );
}
