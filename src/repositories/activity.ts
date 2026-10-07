import { invoke } from "@tauri-apps/api/core";
import { isDebugLoggingEnabled, log, toErrorFields } from "./logging";

export type ActivityKind =
  | "admitted"
  | "queued"
  | "started"
  | "progressed"
  | "replaced"
  | "coalesced"
  | "stale"
  | "paused"
  | "resumed"
  | "stopping"
  | "cancelled"
  | "completed"
  | "failed"
  | "opened"
  | "closed"
  | "changed"
  | "shutdown";

export type ActivityOwner =
  | "app"
  | "section"
  | "selection"
  | "anchor"
  | "viewport"
  | "priority"
  | "sourceCheck"
  | "fileInformation"
  | "backgroundWork"
  | "mutation"
  | "preview"
  | "fullscreen"
  | "comparison"
  | "destination"
  | "settings"
  | "transcript"
  | "managedTools"
  | "media"
  | "watcher"
  | "identity"
  | "delivery";

export type ActivitySubject =
  | "installTools" | "checkToolUpdates"
  | "copyFiles" | "moveFiles" | "deleteFiles" | "emptyDeletedFiles" | "restoreFiles"
  | "previews"
  | "snapshots"
  | "similarity"
  | "faces"
  | "videoTranscription"
  | "audioTranscription";

export type ActivityState =
  | "idle"
  | "queued"
  | "running"
  | "waiting"
  | "stopping"
  | "paused"
  | "enabled"
  | "disabled"
  | "open"
  | "closed"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "stale"
  | "coalesced"
  | "outcomeUnknown";

export type ActivityReason =
  | "user"
  | "sectionChange"
  | "viewportChange"
  | "selectionChange"
  | "priorityChange"
  | "pause"
  | "preemption"
  | "superseded"
  | "staleResponse"
  | "shutdown"
  | "dependency"
  | "completion"
  | "error";

export interface ActivityDraft {
  kind: ActivityKind;
  owner: ActivityOwner;
  subject?: ActivitySubject;
  operationId?: string;
  causeId?: string;
  generation?: number;
  previous?: ActivityState;
  current?: ActivityState;
  reason?: ActivityReason;
  lane?: "image" | "video" | "other";
  itemCount?: number;
  queued?: number;
  done?: number;
  total?: number;
  targetHash?: string;
}

const latestOperations = new Map<ActivityOwner, string>();
let pendingRecord: Promise<unknown> = Promise.resolve();

export function newActivityOperationId(owner: ActivityOwner): string {
  // Every webview has its own JavaScript realm, so a module-local counter can
  // collide across Main, Preview, and Comparison. The UUID keeps operation
  // correlation unique within the Rust-owned app session without an IPC wait.
  const id = `${owner}:${crypto.randomUUID()}`;
  latestOperations.set(owner, id);
  return id;
}

export function latestActivityOperationId(owner: ActivityOwner): string | undefined {
  return latestOperations.get(owner);
}

export function finishActivityOperation(owner: ActivityOwner, operationId: string): void {
  if (latestOperations.get(owner) === operationId) latestOperations.delete(owner);
}

/** Fire-and-forget observation. It must never change product control flow. */
export function recordActivity(draft: ActivityDraft): void {
  if (!isDebugLoggingEnabled() && draft.owner !== "managedTools" && draft.owner !== "settings") return;
  // Preserve each renderer's lifecycle order across asynchronously dispatched
  // native commands without making product work wait for diagnostic I/O.
  pendingRecord = pendingRecord.then(() => invoke("activity_record", { draft }))
    .catch((error) => log.warn("activity recording failed", toErrorFields(error)));
}
