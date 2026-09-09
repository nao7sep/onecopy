import { useEffect, useMemo, useRef, useState } from "react";
import {
  loadActivityPage,
  type ActivityEvent,
} from "../repositories";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";
import PassiveScrollRegion from "./ui/PassiveScrollRegion";

interface ActivitySpan {
  key: string;
  owner: string;
  operationId?: string;
  events: ActivityEvent[];
}

function titleCase(value: string): string {
  return value.replace(/([A-Z])/g, " $1").replace(/^./, (letter) => letter.toUpperCase());
}

export function formatActivityTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const pad = (part: number, width = 2) => String(part).padStart(width, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${pad(date.getMilliseconds(), 3)}`;
}

export function groupActivityEvents(events: ActivityEvent[]): ActivitySpan[] {
  const groups = new Map<string, ActivitySpan>();
  for (const event of [...events].sort((a, b) => b.eventId - a.eventId)) {
    const key = event.operationId === undefined
      ? `event:${event.eventId}`
      : `${event.sessionId}:operation:${event.operationId}`;
    const existing = groups.get(key);
    if (existing === undefined) {
      groups.set(key, {
        key,
        owner: event.owner,
        operationId: event.operationId,
        events: [event],
      });
    } else {
      existing.events.push(event);
    }
  }
  return [...groups.values()];
}

function counts(event: ActivityEvent): string[] {
  return [
    event.itemCount === undefined ? null : `${event.itemCount} items`,
    event.queued === undefined ? null : `${event.queued} queued`,
    event.done === undefined || event.total === undefined
      ? null
      : `${event.done}/${event.total}`,
  ].filter((part): part is string => part !== null);
}

function duration(span: ActivitySpan): string | null {
  const newest = span.events[0];
  const oldest = span.events[span.events.length - 1];
  if (newest === undefined || oldest === undefined || newest.sessionId !== oldest.sessionId) return null;
  const elapsed = newest.monotonicMs - oldest.monotonicMs;
  if (elapsed <= 0) return null;
  return elapsed < 1_000 ? `${elapsed} ms` : `${(elapsed / 1_000).toFixed(1)} s`;
}

function rawEventLine(event: ActivityEvent): string {
  const transition =
    event.previous !== undefined || event.current !== undefined
      ? `${event.previous ?? "—"} → ${event.current ?? "—"}`
      : null;
  return [titleCase(event.kind), transition, ...counts(event)].filter(Boolean).join(" · ");
}

const PAGE_SIZE = 100;
const COPY_FEEDBACK_MS = 2_000;

export default function ActivityTraceModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const [events, setEvents] = useState<ActivityEvent[]>([]);
  const [nextCursor, setNextCursor] = useState<number | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<number | null>(null);

  useEffect(() => {
    if (!open) return;
    let active = true;
    setEvents([]);
    setNextCursor(null);
    setError(null);
    const loadNewest = () => {
      void loadActivityPage(null, PAGE_SIZE)
        .then((page) => {
          if (!active) return;
          setEvents((current) => {
            const byId = new Map(current.map((event) => [event.eventId, event]));
            for (const event of page.events) byId.set(event.eventId, event);
            return [...byId.values()].sort((a, b) => b.eventId - a.eventId);
          });
          setNextCursor((current) => current ?? page.nextCursor);
          setError(null);
        })
        .catch(() => {
          if (active) setError("Activity history could not be loaded.");
        });
    };
    loadNewest();
    const timer = window.setInterval(loadNewest, 1_000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [open]);

  useEffect(() => () => {
    if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
  }, []);

  const spans = useMemo(() => groupActivityEvents(events), [events]);
  const exportText = useMemo(
    () => [...events].reverse().map((event) => JSON.stringify(event)).join("\n"),
    [events],
  );

  const loadOlder = async () => {
    if (loadingMore || nextCursor === null) return;
    setLoadingMore(true);
    try {
      const page = await loadActivityPage(nextCursor, PAGE_SIZE);
      setEvents((current) => [...current, ...page.events]);
      setNextCursor(page.nextCursor);
      setError(null);
    } catch {
      setError("Older activity could not be loaded. Scroll down to try again.");
    } finally {
      setLoadingMore(false);
    }
  };

  if (!open) return null;
  return (
    <ModalShell
      title="Activity trace"
      onClose={onClose}
      widthClass="w-[min(920px,calc(100vw-3rem))]"
      footerResult={error === null ? undefined : <OperationResult level="error">{error}</OperationResult>}
      footerStart={
        <span className="text-xs text-ink-muted">
          {`${events.length} events loaded${nextCursor === null ? "" : " · scroll for older activity"}`}
        </span>
      }
      primaryAction={
        <Button
          disabled={exportText === ""}
          onClick={() => {
            const clipboard = navigator.clipboard;
            if (clipboard === undefined) {
              setError("Copy is unavailable in this window.");
              return;
            }
            void clipboard.writeText(exportText)
              .then(() => {
                setCopied(true);
                if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
                copyTimer.current = window.setTimeout(() => setCopied(false), COPY_FEEDBACK_MS);
              })
              .catch(() => setError("Couldn’t copy the activity trace."));
          }}
        >
          {copied ? "Copied JSONL" : "Copy JSONL"}
        </Button>
      }
    >
      <p className="mb-3 text-xs text-ink-muted">
        Developer-only causal history. Related lifecycle events are combined; expand one to inspect its details.
      </p>
      {events.length === 0 ? (
        <p className="py-8 text-center text-sm text-ink-muted">No activity recorded yet.</p>
      ) : (
        <PassiveScrollRegion
          label="Activity history"
          className="max-h-[58vh]"
          onScroll={(event) => {
            const target = event.currentTarget;
            if (target.scrollHeight - target.scrollTop - target.clientHeight < 120) {
              void loadOlder();
            }
          }}
        >
          <ol className="space-y-2 font-mono text-xs">
            {spans.map((span) => {
              const latest = span.events[0];
              const elapsed = duration(span);
              return (
                <li key={span.key}>
                  <details className="rounded-lg border border-border bg-surface">
                    <summary className="cursor-pointer list-none px-3 py-2.5 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary-ring">
                      <div className="flex items-start gap-3 text-ink">
                        <span className="min-w-0 flex-1">
                          {titleCase(latest.subject ?? span.owner)} · {titleCase(latest.kind)}
                          {span.events.length > 1 ? ` · ${span.events.length} events` : ""}
                          {counts(latest).length > 0 ? ` · ${counts(latest).join(" · ")}` : ""}
                          {elapsed === null ? "" : ` · ${elapsed}`}
                        </span>
                        <time className="shrink-0 text-ink-muted" dateTime={latest.eventTimeUtc}>
                          {formatActivityTime(latest.eventTimeUtc)}
                        </time>
                      </div>
                    </summary>
                    <ol className="space-y-1 border-t border-border px-3 py-2">
                      {[...span.events].reverse().map((event) => (
                        <li key={event.eventId} className="grid grid-cols-[minmax(0,1fr)_auto] gap-3 py-1 text-ink-muted">
                          <span className="min-w-0 break-words">
                            <span className="block">{rawEventLine(event)}</span>
                            {event.operationId !== undefined || event.causeId !== undefined ? (
                              <span className="mt-0.5 block text-ink-muted">
                                {event.operationId === undefined ? null : `operation ${event.operationId}`}
                                {event.operationId !== undefined && event.causeId !== undefined ? " · " : null}
                                {event.causeId === undefined ? null : `caused by ${event.causeId}`}
                              </span>
                            ) : null}
                          </span>
                          <time dateTime={event.eventTimeUtc}>{formatActivityTime(event.eventTimeUtc)}</time>
                        </li>
                      ))}
                    </ol>
                  </details>
                </li>
              );
            })}
          </ol>
          {loadingMore ? <p className="py-3 text-center text-xs text-ink-muted">Loading older activity…</p> : null}
        </PassiveScrollRegion>
      )}
    </ModalShell>
  );
}
