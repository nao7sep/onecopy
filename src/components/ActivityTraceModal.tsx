import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { loadActivityPage, loadActivityEvents, type ActivityOperation, type ActivityEvent } from "../repositories/activity";
import { log, toErrorFields } from "../repositories/logging";
import { activityLabel, formatActivityTime, mergeActivity, operationPresentation, type ActivityText } from "../models/activity-history";
import { captureReadingPosition, restoreReadingPosition, type ReadingPosition } from "../utils/activityReadingPosition";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";
import PassiveScrollRegion from "./ui/PassiveScrollRegion";
import { revealInMain } from "../workflows/reveal-in-main";
import { useDisplayZone } from "../hooks/useDisplayZone";
import { useI18n } from "../i18n/I18nContext";
import type { MessageKey } from "../i18n/catalogues";
import type { Translator } from "../i18n/translate";

export { formatActivityTime } from "../models/activity-history";

/** An enum token the catalogue does not name arrives already spelled out. */
function words(value: ActivityText, text: Translator["text"]): string {
  return typeof value === "string" ? value : text(value);
}

function OperationDetails({ row, beforeChange, afterChange }: {
  row: ActivityOperation; beforeChange: () => void; afterChange: () => void;
}) {
  const { t, text } = useI18n();
  const [events, setEvents] = useState<ActivityEvent[]>([]);
  const [cursor, setCursor] = useState<number | null>(null);
  // The key, not a finished sentence, so the message follows a language change.
  const [error, setError] = useState<MessageKey | null>(null);
  const [refreshError, setRefreshError] = useState<MessageKey | null>(null);
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
      if (alive.current && ticket === request.current) setError("activity.detailsFailed");
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
        if (active) setRefreshError("activity.detailsRefreshFailed");
      }
      if (active) timer = setTimeout(() => void refresh(), 1000);
    };
    void refresh();
    return () => { active = false; clearTimeout(timer); };
  }, [row.id]);
  useLayoutEffect(afterChange, [events]);
  return <div className="space-y-3 border-t border-border px-3 py-3 text-xs">
    <dl data-activity-anchor={"details:" + row.id} className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-ink-muted">
      <dt>{t("activity.session")}</dt><dd className="break-all font-mono">{row.first.sessionId}</dd>
      {row.first.operationId && <><dt>{t("activity.operation")}</dt><dd className="break-all font-mono">{row.first.operationId}</dd></>}
      {row.first.causeId && <><dt>{t("activity.causedBy")}</dt><dd className="break-all font-mono">{row.first.causeId}</dd></>}
      {row.targetHash && <><dt>{t("activity.contentIdentity")}</dt><dd className="break-all font-mono">{row.targetHash}</dd></>}
      <dt>{t("activity.events")}</dt><dd>{row.eventCount}</dd>
    </dl>
    <ol className="space-y-2">
      {events.map((event) => <li key={event.eventId} data-activity-anchor={"event:" + event.eventId} className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1 text-ink-muted">
        <span>{words(activityLabel(event.kind), text)}{event.previous || event.current ? " · " + t("activity.change", {
          previous: event.previous ?? t("activity.unknownValue"),
          current: event.current ?? t("activity.unknownValue"),
        }) : ""}
          {event.reason ? " · " + words(activityLabel(event.reason), text) : ""}
          {event.done !== undefined && event.total !== undefined ? " · " + event.done + "/" + event.total : ""}
          {event.generation !== undefined ? " · " + t("activity.generation", { number: event.generation }) : ""}
          {event.causeId && event.causeId !== row.first.causeId ? " · " + t("activity.eventCausedBy", { id: event.causeId }) : ""}
        </span>
        <time dateTime={event.eventTimeUtc}>{formatActivityTime(event.eventTimeUtc)}</time>
      </li>)}
    </ol>
    {error && <OperationResult level="error">{t(error)}</OperationResult>}
    {refreshError && <OperationResult level="error">{t(refreshError)}</OperationResult>}
    {(cursor !== null || error !== null) && <Button disabled={loading} onClick={() => void load(cursor)}>{loading ? t("activity.detailsLoading") : t("activity.loadEarlierDetails")}</Button>}
  </div>;
}

export default function ActivityTraceModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  // Unmounting makes close/reopen a hard async ownership boundary.
  return open ? <ActivityHistory onClose={onClose} /> : null;
}

function ActivityHistory({ onClose }: { onClose: () => void }) {
  useDisplayZone();
  const { t, text } = useI18n();
  const [rows, setRows] = useState<ActivityOperation[]>([]);
  const [cursor, setCursor] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [olderLoading, setOlderLoading] = useState(false);
  // Keys, not finished sentences, so the messages follow a language change.
  const [error, setError] = useState<MessageKey | null>(null);
  const [olderError, setOlderError] = useState<MessageKey | null>(null);
  const [revealError, setRevealError] = useState<MessageKey | null>(null);
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
        if (active) { setLoading(false); setError("activity.loadFailed"); }
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
      if (alive.current) setOlderError("activity.olderFailed");
    } finally {
      olderPending.current = false;
      if (alive.current) setOlderLoading(false);
    }
  };

  const problems = [error, olderError, revealError]
    .filter((key): key is MessageKey => key !== null);

  return <ModalShell title={t("activity.title")} onClose={onClose} widthClass="w-[min(920px,calc(100vw-3rem))]"
    footerResult={problems.length > 0 ? <OperationResult level="error">{problems.map((key) => t(key)).join(" ")}</OperationResult> : undefined}
    footerStart={<span className="text-xs text-ink-muted">{[
      t("activity.operationsLoaded", { count: rows.length }),
      cursor === null ? null : t("activity.scrollForOlder"),
    ].filter((part) => part !== null).join(" · ")}</span>}>
    <p className="mb-3 text-xs text-ink-muted">{t("activity.intro")}</p>
    <div style={{ overflowAnchor: "none" }}>
      <PassiveScrollRegion label={t("activity.regionLabel")} className="max-h-[58vh]" viewportRef={viewport} onScroll={(event) => {
        const target = event.currentTarget;
        if (target.scrollHeight - target.scrollTop - target.clientHeight < 120) void loadOlder();
      }}>
        {rows.length === 0 && <p className="py-8 text-center text-sm text-ink-muted">{loading ? t("activity.loading") : error ? t("activity.unavailable") : t("activity.none")}</p>}
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
                  <span className="font-medium text-ink">{words(view.action, text)}</span>
                  <time className="text-xs text-ink-muted" dateTime={row.first.eventTimeUtc}>{formatActivityTime(row.first.eventTimeUtc)}</time>
                </span>
                <span className="mt-1 block text-xs text-ink-muted">{[
                  words(view.state, text),
                  view.progress === null ? null : text(view.progress),
                  view.duration,
                ].filter(Boolean).join(" · ")}</span>
              </button>
              {row.targetHash && <div className="px-3 pb-2 text-xs">
                {row.target ? <button type="button" className="break-all text-link hover:underline" onClick={() => {
                  setRevealError(null);
                  void revealInMain(row.target!.path, () => alive.current, onClose, row.targetHash!).then((result) => {
                    if (alive.current && result !== "revealed" && result !== "superseded") setRevealError(result === "blocked"
                      ? "reveal.inMainBlocked"
                      : "reveal.inMainUnavailable");
                  }).catch((error) => {
                    log.warn("activity target reveal failed", toErrorFields(error));
                    if (alive.current) setRevealError("reveal.inMainFailed");
                  });
                }}>{row.target.name}</button> : <span className="text-ink-muted">{t("activity.targetUnavailable")}</span>}
              </div>}
              {isExpanded && <OperationDetails row={row} beforeChange={capture} afterChange={restore} />}
            </li>;
          })}
        </ol>
        {olderLoading && <p className="py-3 text-center text-xs text-ink-muted">{t("activity.loadingOlder")}</p>}
      </PassiveScrollRegion>
    </div>
  </ModalShell>;
}
