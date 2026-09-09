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

const actions: Record<string, string> = {
  app: "OneCopy", sourceCheck: "Check source folders", fileInformation: "Complete file information",
  previews: "Prepare thumbnails and previews", snapshots: "Prepare video snapshots", similarity: "Find similar photos",
  faces: "Score faces", videoTranscription: "Transcribe video", audioTranscription: "Transcribe audio",
  mutation: "File operation", managedTools: "Managed tools", settings: "Apply settings",
  transcript: "Transcription", backgroundWork: "Background work",
  copyFiles: "Copy files", moveFiles: "Move files", deleteFiles: "Delete files", emptyDeletedFiles: "Empty deleted files",
  installTools: "Install managed tool", checkToolUpdates: "Check managed-tool updates",
};

export function activityLabel(value: string): string {
  return actions[value] ?? value.replace(/([A-Z])/g, " $1").replace(/^./, (letter) => letter.toUpperCase());
}

export function operationPresentation(row: ActivityOperation, sessionId: string, nowMs: number) {
  const event = row.latest;
  const running = ["running", "queued", "waiting", "stopping"].includes(event.current ?? "")
    && !["completed", "failed", "cancelled", "closed", "shutdown"].includes(event.kind);
  const state = running && event.sessionId !== sessionId ? "Interrupted — previous app run"
    : ({ running: "Working", queued: "Queued", waiting: "Waiting", stopping: "Stopping", succeeded: "Completed",
      failed: "Failed", cancelled: "Cancelled", paused: "Paused", stale: "Superseded", coalesced: "Combined" } as Record<string, string>)[event.current ?? ""]
      ?? (event.kind === "closed" ? "Ended — outcome not recorded" : activityLabel(event.kind));
  const elapsed = row.started === null ? null : Math.max(0,
    (running && event.sessionId === sessionId ? nowMs : event.monotonicMs) - row.started.monotonicMs);
  const duration = elapsed === null ? null : elapsed < 1000 ? `${elapsed} ms` : `${(elapsed / 1000).toFixed(1)} s`;
  const counts = row.progress?.done !== undefined ? row.progress : event;
  const progress = counts.done !== undefined && counts.total !== undefined ? `${counts.done} / ${counts.total}`
    : (row.progress?.itemCount ?? event.itemCount) !== undefined ? `${row.progress?.itemCount ?? event.itemCount} items` : null;
  return { action: activityLabel(row.first.subject ?? event.subject ?? event.owner), state, duration, progress };
}
