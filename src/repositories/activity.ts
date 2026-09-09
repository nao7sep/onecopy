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
  | "quickView"
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
  | "copyFiles" | "moveFiles" | "deleteFiles" | "emptyDeletedFiles"
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
  | "coalesced";

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

export interface ActivityEvent extends ActivityDraft {
  eventId: number;
  sessionId: string;
  sequence: number;
  eventTimeUtc: string;
  monotonicMs: number;
}

export interface ActivityEventPage {
  debugEnabled: boolean;
  sessionId: string | null;
  monotonicNowMs: number;
  events: ActivityEvent[];
  nextCursor: number | null;
}

export interface ActivityOperation {
  id: number;
  first: ActivityEvent;
  latest: ActivityEvent;
  started: ActivityEvent | null;
  progress: ActivityEvent | null;
  eventCount: number;
  targetHash: string | null;
  target: { name: string; path: string } | null;
}

export interface ActivityPage {
  operations: ActivityOperation[];
  nextCursor: number | null;
  revision: number;
  hasMore: boolean;
  sessionId: string;
  monotonicNowMs: number;
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

export function loadActivityPage(
  before: number | null = null,
  limit = 100,
  after: number | null = null,
): Promise<ActivityPage> {
  return invoke<ActivityPage>("activity_page", { before, after, limit });
}

export function loadActivityEvents(operation: number, before: number | null = null): Promise<ActivityEventPage> {
  return invoke("activity_events", { operation, before, limit: 100 });
}
