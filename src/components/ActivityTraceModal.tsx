import { useEffect, useMemo, useState } from "react";
import {
  loadActivitySnapshot,
  type ActivityEvent,
  type ActivitySnapshot,
} from "../repositories";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";

function titleCase(value: string): string {
  return value.replace(/([A-Z])/g, " $1").replace(/^./, (letter) => letter.toUpperCase());
}

function eventLine(event: ActivityEvent): string {
  const transition =
    event.previous !== undefined || event.current !== undefined
      ? ` ${event.previous ?? "—"} → ${event.current ?? "—"}`
      : "";
  const counts = [
    event.itemCount === undefined ? null : `${event.itemCount} items`,
    event.queued === undefined ? null : `${event.queued} queued`,
    event.done === undefined || event.total === undefined
      ? null
      : `${event.done}/${event.total}`,
  ].filter((part): part is string => part !== null);
  return `${titleCase(event.owner)} · ${titleCase(event.kind)}${transition}${
    counts.length > 0 ? ` · ${counts.join(" · ")}` : ""
  }`;
}

export default function ActivityTraceModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const [snapshot, setSnapshot] = useState<ActivitySnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copyStatus, setCopyStatus] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let active = true;
    const load = () => {
      void loadActivitySnapshot()
        .then((next) => {
          if (active) {
            setSnapshot(next);
            setError(null);
          }
        })
        .catch(() => {
          if (active) setError("Activity history could not be loaded.");
        });
    };
    load();
    const timer = window.setInterval(load, 750);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [open]);

  const exportText = useMemo(
    () => (snapshot === null ? "" : snapshot.events.map((event) => JSON.stringify(event)).join("\n")),
    [snapshot],
  );
  const current = useMemo(() => {
    if (snapshot === null) return [];
    const latest = new Map<string, ActivityEvent>();
    for (const event of [...snapshot.events].reverse()) {
      const key = event.operationId ?? event.owner;
      if (!latest.has(key)) latest.set(key, event);
    }
    return [...latest.values()].filter((event) =>
      event.current === "running" ||
      event.current === "queued" ||
      event.current === "waiting" ||
      event.current === "stopping" ||
      event.current === "paused",
    );
  }, [snapshot]);

  if (!open) return null;
  const events = snapshot?.events ?? [];
  return (
    <ModalShell
      title="Activity trace"
      onClose={onClose}
      widthClass="w-[min(880px,calc(100vw-3rem))]"
      footerStart={
        <span className="text-xs text-ink-muted">
          {error ?? copyStatus ?? `${events.length} retained event${events.length === 1 ? "" : "s"}`}
        </span>
      }
      primaryAction={
        <Button
          disabled={exportText === ""}
          onClick={() => {
            setCopyStatus(null);
            const clipboard = navigator.clipboard;
            if (clipboard === undefined) {
              setCopyStatus("Copy is unavailable in this window.");
              return;
            }
            void clipboard.writeText(exportText)
              .then(() => setCopyStatus("Copied JSONL."))
              .catch(() => setCopyStatus("Couldn’t copy the activity trace."));
          }}
        >
          Copy JSONL
        </Button>
      }
    >
      <p className="mb-3 text-xs text-ink-muted">
        Developer-only causal history. Session {snapshot?.sessionId ?? "starting"}; newest first.
      </p>
      {current.length > 0 ? (
        <section className="mb-4 rounded-xl border border-border bg-surface-muted/40 p-3">
          <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            Current owners
          </h2>
          <ul className="space-y-1 text-xs text-ink">
            {current.map((event) => (
              <li key={event.operationId ?? event.owner}>
                {titleCase(event.owner)} · {titleCase(event.current ?? "idle")} · last event {Math.max(0, (snapshot?.monotonicNowMs ?? 0) - event.monotonicMs)} ms ago
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      {events.length === 0 ? (
        <p className="py-8 text-center text-sm text-ink-muted">No activity recorded yet.</p>
      ) : (
        <ol className="space-y-1 font-mono text-xs">
          {[...events].reverse().map((event) => (
            <li key={event.sequence} className="rounded-lg border border-border px-3 py-2">
              <div className="flex gap-3 text-ink">
                <span className="w-14 shrink-0 text-right text-ink-muted">#{event.sequence}</span>
                <span className="min-w-0 flex-1">{eventLine(event)}</span>
                <span className="shrink-0 text-ink-muted">+{event.monotonicMs} ms</span>
              </div>
              {event.operationId !== undefined || event.causeId !== undefined ? (
                <div className="ml-[4.25rem] mt-1 text-ink-muted">
                  {event.operationId === undefined ? null : `operation ${event.operationId}`}
                  {event.operationId !== undefined && event.causeId !== undefined ? " · " : null}
                  {event.causeId === undefined ? null : `caused by ${event.causeId}`}
                </div>
              ) : null}
            </li>
          ))}
        </ol>
      )}
    </ModalShell>
  );
}
