import { useEffect } from "react";
import { reasonText } from "../models/workReasons";
import { Pause, Play, Square } from "lucide-react";
import {
  backgroundClassLabel,
  backgroundRows,
  backgroundRowCanResume,
  type BackgroundClassSnapshot,
  useDerivedWorkStore,
} from "../state/derived-work-store";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import { useSectionsStore } from "../state/sections-store";
import OperationResult from "./ui/OperationResult";
import { progressLine } from "../models/scan";
import { useI18n } from "../i18n/I18nContext";
import type { Translator } from "../i18n/translate";

function stateText(row: BackgroundClassSnapshot, t: Translator["t"]): string {
  switch (row.state) {
    case "disabled":
      return reasonText(row.reason, t) ?? t("work.offInSettings");
    case "unavailable":
      return reasonText(row.reason, t) ?? t("work.toolUnavailable");
    case "queued":
      return t("work.queuedCount", { count: row.queued });
    case "waiting":
      return reasonText(row.reason, t) ?? t("work.waitingCount", { count: row.queued });
    case "running":
      return row.done !== null && row.total !== null
        ? t("work.runningProgress", { done: row.done, total: row.total })
        : t("work.running");
    case "stopping":
      return t("work.stopping");
    case "paused":
      return t("work.queuedCountPaused", { count: row.queued });
    case "failed":
    case "up-to-date":
      return t("work.noWork");
  }
}

// Keyed by class id, so a new class cannot reach the list without a description.
const DESCRIPTIONS = {
  previews: "work.previewsDescription",
  snapshots: "work.snapshotsDescription",
  similarity: "work.similarityDescription",
  faces: "work.facesDescription",
  "video-transcripts": "work.videoTranscriptsDescription",
  "audio-transcripts": "work.audioTranscriptsDescription",
} as const;

export default function BackgroundWorkModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const { t, text, percent } = useI18n();
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
  const failure = indexError ?? error;
  const rows = snapshot === null ? [] : backgroundRows(snapshot);
  const allPaused = fileInformation.paused && rows.every((row) =>
    row.state === "disabled" || snapshot?.pausedClasses.includes(row.id));

  return (
    <ModalShell
      title={t("work.title")}
      onClose={onClose}
      widthClass="w-[min(680px,calc(100vw-3rem))]"
      footerResult={
        failure !== null ? (
          <OperationResult level="error">{text(failure)}</OperationResult>
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
            {t("work.pauseAll")}
          </Button>
        ) : undefined
      }
    >
      <p className="mb-4 text-sm text-ink-muted">
        {t("work.intro")}
      </p>
      <ul className="mb-4 space-y-2">
        <li className="flex items-center gap-4 rounded-xl border border-border bg-surface-muted/40 px-4 py-3">
          <span className="min-w-0 flex-1">
            <span className="block text-sm font-semibold text-ink-strong">
              {t("work.sourceCheck")}
            </span>
            <span className="mt-0.5 block text-xs text-ink-muted">
              {t("work.sourceCheckDescription")}
            </span>
            <span className="mt-1 block text-xs text-ink">
              {sourceCheck.stopping
                ? t("work.sourceCheckStopping")
                : sourceCheck.waiting
                  ? t("work.sourceCheckWaiting")
                : sourceCheck.running
                  ? sourceCheck.progress === null ? t("work.running") : progressLine(sourceCheck.progress, t, percent)
                  : sourceCheck.lastResult === "completed"
                    ? t("work.sourceCheckCompleted")
                    : sourceCheck.lastResult === "completed-with-issues"
                      ? t("work.sourceCheckIncomplete")
                    : sourceCheck.lastResult === "failed"
                      ? t("work.sourceCheckFailed")
                      : t("work.stopped")}
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
            {sourceCheck.running ? t("work.stop") : t("work.start")}
          </Button>
        </li>
        <li className="flex items-center gap-4 rounded-xl border border-border bg-surface-muted/40 px-4 py-3">
          <span className="min-w-0 flex-1">
            <span className="block text-sm font-semibold text-ink-strong">
              {t("work.fileInformation")}
            </span>
            <span className="mt-0.5 block text-xs text-ink-muted">
              {t("work.fileInformationDescription")}
            </span>
            <span className="mt-1 block text-xs text-ink">
              {fileInformation.stopping
                ? t("work.fileInformationPausing")
                : fileInformation.paused
                  ? fileInformation.queued
                    ? t("work.workQueuedPaused")
                    : t("work.paused")
                  : fileInformation.running
                  ? fileInformation.progress === null ? t("work.running") : progressLine(fileInformation.progress, t, percent)
                    : fileInformation.queued
                      ? t("work.queued")
                      : t("work.noWork")}
            </span>
          </span>
          <Button
            size="sm"
            disabled={fileInformation.stopping}
            onClick={() => void setFileInformationPaused(!fileInformation.paused)}
          >
            {fileInformation.paused ? <Play size={13} /> : <Pause size={13} />}
            {fileInformation.paused ? t("work.resume") : t("work.pause")}
          </Button>
        </li>
      </ul>
      <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-ink-muted">
        {t("work.enrichmentHeading")}
      </h2>
      {snapshot !== null && !snapshot.workerRunning ? (
        <p className="mb-3 text-sm text-ink-muted">
          {t("work.workerStopped")}
        </p>
      ) : null}
      {snapshot === null ? (
        <p className="py-6 text-center text-sm text-ink-muted">
          {loading ? t("work.loading") : t("work.unavailable")}
        </p>
      ) : (
        <ul className="space-y-2">
          {rows.map((row) => {
            const canResume = backgroundRowCanResume(snapshot, row);
            const rowChanging = changing === row.id;
            return (
              <li
                key={row.id}
                className="flex items-center gap-4 rounded-xl border border-border bg-surface-muted/40 px-4 py-3"
              >
                <span className="min-w-0 flex-1">
                  <span className="block text-sm font-semibold text-ink-strong">
                    {t(backgroundClassLabel(row.id))}
                  </span>
                  <span className="mt-0.5 block text-xs text-ink-muted">
                    {t(DESCRIPTIONS[row.id])}
                  </span>
                  <span
                    className={`mt-1 block text-xs ${
                      row.state === "unavailable" ? "text-warning" : "text-ink"
                    }`}
                  >
                    {!snapshot.workerRunning && row.state === "queued"
                      ? t("work.stopped")
                      : stateText(row, t)}
                  </span>
                </span>
                <Button
                  size="sm"
                  disabled={
                    changing !== null ||
                    row.state === "disabled" ||
                    row.state === "stopping"
                  }
                  onClick={() => void setPaused(row.id, !canResume)}
                >
                  {canResume ? <Play size={13} /> : <Pause size={13} />}
                  {rowChanging
                    ? t("common.saving")
                    : canResume
                      ? t("work.resume")
                      : t("work.pause")}
                </Button>
              </li>
            );
          })}
        </ul>
      )}
    </ModalShell>
  );
}
