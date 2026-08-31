import { useEffect, useRef } from "react";
import { X } from "lucide-react";
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

function Toast({
  record,
  durationMs,
}: {
  record: NotificationRecord;
  durationMs: number;
}) {
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
      className={`pointer-events-auto w-[min(420px,calc(100vw-2rem))] rounded-lg border p-3 shadow-xl ${tone}`}
      onMouseEnter={stopTimer}
      onMouseLeave={startTimer}
      onFocusCapture={stopTimer}
      onBlurCapture={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) startTimer();
      }}
    >
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <p className="select-text break-words text-sm">{record.message}</p>
          {record.path ? (
            <p className="mt-1 select-text break-all text-xs opacity-70">{record.path}</p>
          ) : null}
        </div>
        <button
          aria-label="Dismiss notification"
          title="Dismiss"
          disabled={dismissing}
          className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-current opacity-70 hover:bg-black/10 hover:opacity-100 disabled:opacity-30"
          onClick={() => void dismiss(record.id)}
        >
          <X size={14} />
        </button>
      </div>
      {record.occurrenceCount > 1 ? (
        <p className="mt-1 text-xs opacity-70">Occurred {record.occurrenceCount} times</p>
      ) : null}
    </section>
  );
}

export default function NotificationHost() {
  const active = useNotificationsStore((state) => state.active);
  const configuredSeconds = useAppStore((state) => {
    const value = state.appData?.config?.notificationDisplaySeconds;
    return typeof value === "number" && Number.isFinite(value) ? value : 6;
  });
  const durationMs = Math.min(60, Math.max(1, configuredSeconds)) * 1000;

  useEffect(() => {
    void installNotificationWiring().catch((error) => {
      const message = error instanceof Error ? error.message : String(error);
      const direct = `Notifications are unavailable: ${message}`;
      presentEscapedFailure(direct);
      recordInterfaceFailure(direct);
    });
  }, []);

  if (active.length === 0) return null;
  return (
    <div className="pointer-events-none fixed right-4 top-4 z-[25] flex flex-col items-end gap-2">
      {active.map((record) => (
        <Toast key={record.id} record={record} durationMs={durationMs} />
      ))}
    </div>
  );
}
