import { Pause, Play, Square } from "lucide-react";
import {
  backgroundClassLabel,
  type BackgroundClassSnapshot,
  useDerivedWorkStore,
} from "../state/derived-work-store";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import { useSectionsStore } from "../state/sections-store";

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
      return `${row.failed.toLocaleString()} failed — open Issues to retry`;
    case "up-to-date":
      return "Up to date";
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

export default function BackgroundWorkModal() {
  const open = useDerivedWorkStore((state) => state.open);
  const snapshot = useDerivedWorkStore((state) => state.snapshot);
  const loading = useDerivedWorkStore((state) => state.loading);
  const changing = useDerivedWorkStore((state) => state.changing);
  const error = useDerivedWorkStore((state) => state.error);
  const indexError = useSectionsStore((state) => state.error);
  const setOpen = useDerivedWorkStore((state) => state.setOpen);
  const setPaused = useDerivedWorkStore((state) => state.setPaused);
  const sourceCheck = useSectionsStore((state) => state.sourceCheck);
  const fileInformation = useSectionsStore((state) => state.fileInformation);
  const startSourceCheck = useSectionsStore((state) => state.startSourceCheck);
  const stopSourceCheck = useSectionsStore((state) => state.stopSourceCheck);
  const setFileInformationPaused = useSectionsStore(
    (state) => state.setFileInformationPaused,
  );

  if (!open) return null;

  return (
    <ModalShell
      title="Background work"
      onClose={() => setOpen(false)}
      widthClass="w-[min(680px,calc(100vw-3rem))]"
      footerStart={indexError ?? error}
      primaryAction={
        snapshot !== null ? (
          <Button
            disabled={
              changing !== null || snapshot.classes.some((row) => row.state === "stopping")
            }
            onClick={() => void setPaused(null, !snapshot.masterPaused)}
          >
            {snapshot.masterPaused ? <Play size={14} /> : <Pause size={14} />}
            {snapshot.masterPaused
              ? "Resume preparation and enrichment"
              : "Pause preparation and enrichment"}
          </Button>
        ) : undefined
      }
    >
      <p className="mb-4 text-sm text-ink-muted">
        These jobs may run while you use OneCopy. Stopping or pausing keeps every fact already
        saved and never changes your files.
      </p>
      <ul className="mb-4 space-y-2">
        <li className="flex items-center gap-4 rounded-xl border border-border bg-surface-muted/40 px-4 py-3">
          <span className="min-w-0 flex-1">
            <span className="block text-sm font-semibold text-ink-strong">
              Check source folders
            </span>
            <span className="mt-0.5 block text-xs text-ink-muted">
              Finds files added, removed, or changed since OneCopy last saw each folder.
            </span>
            <span className="mt-1 block text-xs text-ink">
              {sourceCheck.stopping
                ? "Stopping after the current safe step…"
                : sourceCheck.running
                  ? "Running…"
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
                    ? "Running…"
                    : fileInformation.queued
                      ? "Queued"
                      : "Up to date"}
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
          {snapshot.classes.map((row) => {
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
                    {row.failed > 0 && row.state !== "failed"
                      ? ` — ${row.failed.toLocaleString()} failed`
                      : ""}
                  </span>
                </span>
                <Button
                  size="sm"
                  disabled={
                    snapshot.masterPaused ||
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
