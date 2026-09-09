import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { loadActivityPage, loadActivityEvents, type ActivityOperation, type ActivityEvent } from "../repositories/activity";
import { log, toErrorFields } from "../repositories/logging";
import { activityLabel, formatActivityTime, mergeActivity, operationPresentation } from "../models/activity-history";
import { captureReadingPosition, restoreReadingPosition, type ReadingPosition } from "../utils/activityReadingPosition";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";
import PassiveScrollRegion from "./ui/PassiveScrollRegion";
import { revealInMain } from "../workflows/reveal-in-main";
import { useDisplayZone } from "../hooks/useDisplayZone";

export { formatActivityTime } from "../models/activity-history";

function OperationDetails({ row, beforeChange, afterChange }: {
  row: ActivityOperation; beforeChange: () => void; afterChange: () => void;
}) {
  const [events, setEvents] = useState<ActivityEvent[]>([]);
  const [cursor, setCursor] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const alive = useRef(false);
  const request = useRef(0);
  const currentEvents = useRef(events);
  const latest = useRef(row.latest.eventId);
  latest.current = row.latest.eventId;
  currentEvents.current = events;
  useEffect(() => { alive.current = true; return () => { alive.current = false; request.current++; }; }, []);
  const load = async (before: number | null) => {
    const ticket = ++request.current;
    setLoading(true);
    try {
      const page = await loadActivityEvents(row.id, before);
      if (!alive.current || ticket !== request.current) return;
      beforeChange();
      setEvents((current) => {
        const rows = new Map(current.map((event) => [event.eventId, event]));
        for (const event of page.events) rows.set(event.eventId, event);
        return [...rows.values()].sort((a, b) => b.eventId - a.eventId);
      });
      setCursor(page.nextCursor);
      setError(null);
    } catch (error) {
      log.warn("activity details failed", toErrorFields(error));
      if (alive.current && ticket === request.current) setError("Details could not be loaded. Try again.");
    } finally {
      if (alive.current && ticket === request.current) setLoading(false);
    }
  };
  useEffect(() => {
    let active = true;
    let revision: number | null = null;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const refresh = async () => {
      const nextRevision = latest.current;
      if (revision === nextRevision) {
        timer = setTimeout(() => void refresh(), 1000);
        return;
      }
      const head = currentEvents.current[0]?.eventId;
      try {
        let before: number | null = null;
        const incoming: ActivityEvent[] = [];
        do {
          const page = await loadActivityEvents(row.id, before);
          if (!active) return;
          incoming.push(...page.events);
          before = page.nextCursor;
          if (head === undefined || page.events.some((event) => event.eventId <= head)) break;
        } while (before !== null);
        beforeChange();
        setEvents((current) => {
          const merged = new Map(current.map((event) => [event.eventId, event]));
          for (const event of incoming) merged.set(event.eventId, event);
          return [...merged.values()].sort((a, b) => b.eventId - a.eventId);
        });
        if (head === undefined) setCursor(before);
        revision = nextRevision;
        setRefreshError(null);
      } catch (error) {
        log.warn("activity details refresh failed", toErrorFields(error));
        if (active) setRefreshError("Details could not be refreshed. Retrying…");
      }
      if (active) timer = setTimeout(() => void refresh(), 1000);
    };
    void refresh();
    return () => { active = false; clearTimeout(timer); };
  }, [row.id]);
  useLayoutEffect(afterChange, [events]);
  return <div className="space-y-3 border-t border-border px-3 py-3 text-xs">
    <dl data-activity-anchor={"details:" + row.id} className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-ink-muted">
      <dt>Session</dt><dd className="break-all font-mono">{row.first.sessionId}</dd>
      {row.first.operationId && <><dt>Operation</dt><dd className="break-all font-mono">{row.first.operationId}</dd></>}
      {row.first.causeId && <><dt>Caused by</dt><dd className="break-all font-mono">{row.first.causeId}</dd></>}
      {row.targetHash && <><dt>Content identity</dt><dd className="break-all font-mono">{row.targetHash}</dd></>}
      <dt>Events</dt><dd>{row.eventCount}</dd>
    </dl>
    <ol className="space-y-2">
      {events.map((event) => <li key={event.eventId} data-activity-anchor={"event:" + event.eventId} className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1 text-ink-muted">
        <span>{activityLabel(event.kind)}{event.previous || event.current ? " · " + (event.previous ?? "unknown") + " → " + (event.current ?? "unknown") : ""}
          {event.reason ? " · " + activityLabel(event.reason) : ""}
          {event.done !== undefined && event.total !== undefined ? " · " + event.done + "/" + event.total : ""}
          {event.generation !== undefined ? " · generation " + event.generation : ""}
          {event.causeId && event.causeId !== row.first.causeId ? " · caused by " + event.causeId : ""}
        </span>
        <time dateTime={event.eventTimeUtc}>{formatActivityTime(event.eventTimeUtc)}</time>
      </li>)}
    </ol>
    {error && <OperationResult level="error">{error}</OperationResult>}
    {refreshError && <OperationResult level="error">{refreshError}</OperationResult>}
    {(cursor !== null || error !== null) && <Button disabled={loading} onClick={() => void load(cursor)}>{loading ? "Loading…" : "Load earlier details"}</Button>}
  </div>;
}

export default function ActivityTraceModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  // Unmounting makes close/reopen a hard async ownership boundary.
  return open ? <ActivityHistory onClose={onClose} /> : null;
}

function ActivityHistory({ onClose }: { onClose: () => void }) {
  useDisplayZone();
  const [rows, setRows] = useState<ActivityOperation[]>([]);
  const [cursor, setCursor] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [olderLoading, setOlderLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [olderError, setOlderError] = useState<string | null>(null);
  const [revealError, setRevealError] = useState<string | null>(null);
  const [clock, setClock] = useState({ sessionId: "", monotonicNowMs: 0 });
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const viewport = useRef<HTMLDivElement>(null);
  const position = useRef<ReadingPosition | null>(null);
  const alive = useRef(false);
  const olderPending = useRef(false);
  const capture = () => { position.current = captureReadingPosition(viewport.current); };
  const restore = () => { restoreReadingPosition(viewport.current, position.current); position.current = null; };
  useLayoutEffect(restore, [rows, expanded]);

  useEffect(() => {
    alive.current = true;
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let revision: number | null = null;
    const poll = async () => {
      try {
        let more: boolean;
        do {
          const initial = revision === null;
          const page = await loadActivityPage(null, 100, revision);
          if (!active) return;
          capture();
          setRows((current) => mergeActivity(current, page.operations, !initial));
          if (initial) setCursor(page.nextCursor);
          setClock({ sessionId: page.sessionId, monotonicNowMs: page.monotonicNowMs });
          revision = page.revision;
          more = !initial && page.hasMore;
          setError(null);
          setLoading(false);
        } while (more && active);
      } catch (error) {
        log.warn("activity history failed", toErrorFields(error));
        if (active) { setLoading(false); setError("Activity history could not be loaded. Retrying…"); }
      }
      if (active) timer = setTimeout(() => void poll(), 1000);
    };
    void poll();
    return () => { active = false; alive.current = false; clearTimeout(timer); };
  }, []);

  const loadOlder = async () => {
    if (cursor === null || olderPending.current) return;
    olderPending.current = true;
    setOlderLoading(true);
    try {
      const page = await loadActivityPage(cursor);
      if (!alive.current) return;
      capture();
      setRows((current) => mergeActivity(current, page.operations, false));
      setCursor(page.nextCursor);
      setOlderError(null);
    } catch (error) {
      log.warn("older activity failed", toErrorFields(error));
      if (alive.current) setOlderError("Older activity could not be loaded. Scroll down to try again.");
    } finally {
      olderPending.current = false;
      if (alive.current) setOlderLoading(false);
    }
  };

  return <ModalShell title="Activity trace" onClose={onClose} widthClass="w-[min(920px,calc(100vw-3rem))]"
    footerResult={error || olderError || revealError ? <OperationResult level="error">{[error, olderError, revealError].filter(Boolean).join(" ")}</OperationResult> : undefined}
    footerStart={<span className="text-xs text-ink-muted">{rows.length} operations loaded{cursor === null ? "" : " · scroll for older activity"}</span>}>
    <p className="mb-3 text-xs text-ink-muted">Work updates here while it runs. Expand an entry for technical details.</p>
    <div style={{ overflowAnchor: "none" }}>
      <PassiveScrollRegion label="Activity history" className="max-h-[58vh]" viewportRef={viewport} onScroll={(event) => {
        const target = event.currentTarget;
        if (target.scrollHeight - target.scrollTop - target.clientHeight < 120) void loadOlder();
      }}>
        {rows.length === 0 && <p className="py-8 text-center text-sm text-ink-muted">{loading ? "Loading activity…" : error ? "Activity is unavailable." : "No activity recorded yet."}</p>}
        <ol className="space-y-2 text-sm">
          {rows.map((row) => {
            const view = operationPresentation(row, clock.sessionId, clock.monotonicNowMs);
            const isExpanded = expanded.has(row.id);
            return <li key={row.id} className="rounded-lg border border-border bg-surface">
              <button data-activity-anchor={"operation:" + row.id} type="button" aria-expanded={isExpanded} className="w-full rounded-lg px-3 py-2.5 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary-ring" onClick={() => {
                capture();
                setExpanded((current) => { const next = new Set(current); if (next.has(row.id)) next.delete(row.id); else next.add(row.id); return next; });
              }}>
                <span className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
                  <span className="font-medium text-ink">{view.action}</span>
                  <time className="text-xs text-ink-muted" dateTime={row.first.eventTimeUtc}>{formatActivityTime(row.first.eventTimeUtc)}</time>
                </span>
                <span className="mt-1 block text-xs text-ink-muted">{[view.state, view.progress, view.duration].filter(Boolean).join(" · ")}</span>
              </button>
              {row.targetHash && <div className="px-3 pb-2 text-xs">
                {row.target ? <button type="button" className="break-all text-link hover:underline" onClick={() => {
                  setRevealError(null);
                  void revealInMain(row.target!.path, () => alive.current, onClose, row.targetHash!).then((result) => {
                    if (alive.current && result !== "revealed" && result !== "superseded") setRevealError(result === "blocked"
                      ? "Finish the current review or file operation before revealing this file."
                      : "This file is no longer available in Main.");
                  }).catch((error) => {
                    log.warn("activity target reveal failed", toErrorFields(error));
                    if (alive.current) setRevealError("The file could not be revealed in Main.");
                  });
                }}>{row.target.name}</button> : <span className="text-ink-muted">File no longer available in Main</span>}
              </div>}
              {isExpanded && <OperationDetails row={row} beforeChange={capture} afterChange={restore} />}
            </li>;
          })}
        </ol>
        {olderLoading && <p className="py-3 text-center text-xs text-ink-muted">Loading older activity…</p>}
      </PassiveScrollRegion>
    </div>
  </ModalShell>;
}
