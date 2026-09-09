import { useEffect } from "react";
import { Pause, Play, Square } from "lucide-react";
import {
  backgroundClassLabel,
  backgroundRows,
  type BackgroundClassSnapshot,
  useDerivedWorkStore,
} from "../state/derived-work-store";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import { useSectionsStore } from "../state/sections-store";
import OperationResult from "./ui/OperationResult";
import { progressLine } from "../models/scan";

function stateText(row: BackgroundClassSnapshot): string {
  switch (row.state) {
    case "disabled":
      return row.reason ?? "Off in Settings";
    case "unavailable":
      return row.reason ?? "Required tool unavailable";
    case "queued":
      return `${row.queued.toLocaleString()} queued`;
    case "waiting":
      return row.reason ?? `${row.queued.toLocaleString()} waiting`;
    case "running":
      return row.done !== null && row.total !== null
        ? `Running — ${row.done.toLocaleString()}/${row.total.toLocaleString()}`
        : "Running…";
    case "stopping":
      return "Stopping and releasing resources…";
    case "paused":
      return `${row.queued.toLocaleString()} queued — paused`;
    case "failed":
    case "up-to-date":
      return "No work running";
  }
}

const DESCRIPTIONS: Record<BackgroundClassSnapshot["id"], string> = {
  previews: "Screen-sized images and video posters. Visible items go first.",
  snapshots: "Timestamped scene frames for quickly understanding a video.",
  similarity: "Rebuilds similar-photo families after preview facts change.",
  faces: "Optional face and expression scoring used to order comparison groups.",
  "video-transcripts": "Optional speech-to-text for videos with audio.",
  "audio-transcripts": "Optional speech-to-text for audio files.",
};

export default function BackgroundWorkModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const snapshot = useDerivedWorkStore((state) => state.snapshot);
  const loading = useDerivedWorkStore((state) => state.loading);
  const changing = useDerivedWorkStore((state) => state.changing);
  const error = useDerivedWorkStore((state) => state.error);
  const indexError = useSectionsStore((state) => state.error);
  const setPaused = useDerivedWorkStore((state) => state.setPaused);
  const sourceCheck = useSectionsStore((state) => state.sourceCheck);
  const fileInformation = useSectionsStore((state) => state.fileInformation);
  const startSourceCheck = useSectionsStore((state) => state.startSourceCheck);
  const stopSourceCheck = useSectionsStore((state) => state.stopSourceCheck);
  const setFileInformationPaused = useSectionsStore(
    (state) => state.setFileInformationPaused,
  );

  useEffect(() => {
    if (open) void useDerivedWorkStore.getState().load();
  }, [open]);

  if (!open) return null;
  const rows = snapshot === null ? [] : backgroundRows(snapshot);
  const allPaused = fileInformation.paused && rows.every((row) =>
    row.state === "disabled" || snapshot?.pausedClasses.includes(row.id));

  return (
    <ModalShell
      title="Background work"
      onClose={onClose}
      widthClass="w-[min(680px,calc(100vw-3rem))]"
      footerStart={
        indexError !== null || error !== null ? (
          <OperationResult level="error">{indexError ?? error}</OperationResult>
        ) : undefined
      }
      primaryAction={
        snapshot !== null ? (
          <Button
            disabled={
              changing !== null || allPaused
            }
            onClick={() => void setPaused(null, true)}
          >
            <Pause size={14} />
            Pause all
          </Button>
        ) : undefined
      }
    >
      <p className="mb-4 text-sm text-ink-muted">
        Pausing keeps completed work and never changes your files. These controls apply to this
        app session; Settings decides which optional features are enabled.
        {" "}Pause all pauses file information, preparation, and enrichment. Source checking has
        its own Stop control; folder watching stays active.
      </p>
      <ul className="mb-4 space-y-2">
        <li className="flex items-center gap-4 rounded-xl border border-border bg-surface-muted/40 px-4 py-3">
          <span className="min-w-0 flex-1">
            <span className="block text-sm font-semibold text-ink-strong">
              Check source folders
            </span>
            <span className="mt-0.5 block text-xs text-ink-muted">
              One pass to find added, removed, or changed files. Folder watching continues afterward.
            </span>
            <span className="mt-1 block text-xs text-ink">
              {sourceCheck.stopping
                ? "Stopping after the current safe step…"
                : sourceCheck.waiting
                  ? "Waiting for the current file operation…"
                : sourceCheck.running
                  ? sourceCheck.progress === null ? "Running…" : progressLine(sourceCheck.progress)
                  : sourceCheck.lastResult === "completed"
                    ? "Completed — Start checks again"
                    : sourceCheck.lastResult === "completed-with-issues"
                      ? "Finished with incomplete checks"
                    : sourceCheck.lastResult === "failed"
                      ? "Could not complete check"
                      : "Stopped"}
            </span>
          </span>
          <Button
            size="sm"
            disabled={sourceCheck.stopping}
            onClick={() =>
              void (sourceCheck.running ? stopSourceCheck() : startSourceCheck())
            }
          >
            {sourceCheck.running ? <Square size={13} /> : <Play size={13} />}
            {sourceCheck.running ? "Stop" : "Start"}
          </Button>
        </li>
        <li className="flex items-center gap-4 rounded-xl border border-border bg-surface-muted/40 px-4 py-3">
          <span className="min-w-0 flex-1">
            <span className="block text-sm font-semibold text-ink-strong">
              Complete file information
            </span>
            <span className="mt-0.5 block text-xs text-ink-muted">
              Completes missing identity, metadata, dates, and companion relationships.
            </span>
            <span className="mt-1 block text-xs text-ink">
              {fileInformation.stopping
                ? "Pausing after the current safe step…"
                : fileInformation.paused
                  ? fileInformation.queued
                    ? "Work queued — paused"
                    : "Paused"
                  : fileInformation.running
                  ? fileInformation.progress === null ? "Running…" : progressLine(fileInformation.progress)
                    : fileInformation.queued
                      ? "Queued"
                      : "No work running"}
            </span>
          </span>
          <Button
            size="sm"
            disabled={fileInformation.stopping}
            onClick={() => void setFileInformationPaused(!fileInformation.paused)}
          >
            {fileInformation.paused ? <Play size={13} /> : <Pause size={13} />}
            {fileInformation.paused ? "Resume" : "Pause"}
          </Button>
        </li>
      </ul>
      <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-ink-muted">
        Preparation and enrichment
      </h2>
      {snapshot === null ? (
        <p className="py-6 text-center text-sm text-ink-muted">
          {loading ? "Reading background work…" : "Background-work status is unavailable."}
        </p>
      ) : (
        <ul className="space-y-2">
          {rows.map((row) => {
            const paused = row.state === "paused" || row.state === "stopping";
            const rowChanging = changing === row.id;
            return (
              <li
                key={row.id}
                className="flex items-center gap-4 rounded-xl border border-border bg-surface-muted/40 px-4 py-3"
              >
                <span className="min-w-0 flex-1">
                  <span className="block text-sm font-semibold text-ink-strong">
                    {backgroundClassLabel(row.id)}
                  </span>
                  <span className="mt-0.5 block text-xs text-ink-muted">
                    {DESCRIPTIONS[row.id]}
                  </span>
                  <span
                    className={`mt-1 block text-xs ${
                      row.state === "unavailable" ? "text-warning" : "text-ink"
                    }`}
                  >
                    {stateText(row)}
                  </span>
                </span>
                <Button
                  size="sm"
                  disabled={
                    changing !== null ||
                    row.state === "disabled" ||
                    row.state === "stopping"
                  }
                  onClick={() => void setPaused(row.id, !paused)}
                >
                  {paused ? <Play size={13} /> : <Pause size={13} />}
                  {rowChanging ? "Saving…" : paused ? "Resume" : "Pause"}
                </Button>
              </li>
            );
          })}
        </ul>
      )}
    </ModalShell>
  );
}
