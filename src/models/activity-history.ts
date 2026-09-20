import type { MessageKey } from "../i18n/catalogues";
import { message, type Message } from "../i18n/translate";
import type { ActivityOperation } from "../repositories/activity";

export function mergeActivity(current: ActivityOperation[], incoming: ActivityOperation[], live: boolean): ActivityOperation[] {
  const rows = new Map(current.map((row) => [row.id, row]));
  const oldest = current[current.length - 1]?.id ?? 0;
  for (const row of incoming) {
    // A changed operation below the loaded history boundary must not create a
    // disconnected older island and make ordinary pagination skip the gap.
    if (live && row.id < oldest && !rows.has(row.id)) continue;
    const previous = rows.get(row.id);
    if (previous === undefined || previous.latest.eventId < row.latest.eventId) rows.set(row.id, row);
  }
  return [...rows.values()].sort((a, b) => b.id - a.id);
}

export function formatActivityTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const pad = (part: number, width = 2) => String(part).padStart(width, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${pad(date.getMilliseconds(), 3)}`;
}

/** Either a named action or state, or a diagnostic enum token the catalogue
 * does not name and this module spells out from the token itself. */
export type ActivityText = Message | string;

const ACTION_KEYS: Record<string, MessageKey> = {
  sourceCheck: "work.sourceCheck", fileInformation: "work.fileInformation",
  previews: "activity.actionPreviews", snapshots: "activity.actionSnapshots", similarity: "activity.actionSimilarity",
  faces: "activity.actionFaces", videoTranscription: "activity.actionVideoTranscription",
  audioTranscription: "activity.actionAudioTranscription",
  mutation: "activity.actionMutation", managedTools: "binaries.title", settings: "activity.actionSettings",
  transcript: "activity.actionTranscript", backgroundWork: "work.title",
  copyFiles: "activity.actionCopyFiles", moveFiles: "activity.actionMoveFiles",
  deleteFiles: "activity.actionDeleteFiles", emptyDeletedFiles: "activity.actionEmptyDeletedFiles",
  installTools: "activity.actionInstallTools", checkToolUpdates: "activity.actionCheckToolUpdates",
};

const STATE_KEYS: Record<string, MessageKey> = {
  running: "activity.stateWorking", queued: "work.queued", waiting: "activity.stateWaiting",
  stopping: "activity.stateStopping", succeeded: "activity.stateCompleted", failed: "activity.stateFailed",
  cancelled: "activity.stateCancelled", paused: "work.paused", stale: "activity.stateSuperseded",
  coalesced: "activity.stateCombined",
};

// The app's own name, which reads the same in every language.
const APP_OWNER = "OneCopy";

export function activityLabel(value: string): ActivityText {
  if (value === "app") return APP_OWNER;
  const key = ACTION_KEYS[value];
  return key === undefined
    ? value.replace(/([A-Z])/g, " $1").replace(/^./, (letter) => letter.toUpperCase())
    : message(key);
}

export function operationPresentation(row: ActivityOperation, sessionId: string, nowMs: number) {
  const event = row.latest;
  const running = ["running", "queued", "waiting", "stopping"].includes(event.current ?? "")
    && !["completed", "failed", "cancelled", "closed", "shutdown"].includes(event.kind);
  const stateKey = STATE_KEYS[event.current ?? ""];
  const state: ActivityText = running && event.sessionId !== sessionId
    ? message("activity.stateInterrupted")
    : stateKey !== undefined ? message(stateKey)
      : event.kind === "closed" ? message("activity.stateEnded") : activityLabel(event.kind);
  const elapsed = row.started === null ? null : Math.max(0,
    (running && event.sessionId === sessionId ? nowMs : event.monotonicMs) - row.started.monotonicMs);
  const duration = elapsed === null ? null : elapsed < 1000 ? `${elapsed} ms` : `${(elapsed / 1000).toFixed(1)} s`;
  const counts = row.progress?.done !== undefined ? row.progress : event;
  const itemCount = row.progress?.itemCount ?? event.itemCount;
  const progress: Message | null =
    counts.done !== undefined && counts.total !== undefined
      ? message("activity.progressCount", { done: counts.done, total: counts.total })
      : itemCount !== undefined ? message("activity.itemCount", { count: itemCount }) : null;
  return { action: activityLabel(row.first.subject ?? event.subject ?? event.owner), state, duration, progress };
}
