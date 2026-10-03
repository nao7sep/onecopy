// What the Records window reads from records.sqlite3 (src-tauri/src/
// records_view.rs): a filtered page of summaries, newest first, and one
// record whole. Fields arrive as the database holds them; this module decides
// how each is named and shown.

import type { MessageKey } from "../i18n/catalogues";
import { message, type Message, type MessageValues } from "../i18n/translate";

export type RecordKind = "log" | "activity" | "trash" | "analysis" | "issue" | "notice";

export type RecordLevel = "debug" | "info" | "warn" | "error";

export const RECORD_KINDS: readonly RecordKind[] = ["log", "activity", "trash", "analysis", "issue", "notice"];

export const RECORD_LEVELS: readonly RecordLevel[] = ["error", "warn", "info", "debug"];

// What the level filter offers: a record's own level, or `attention`, every
// record at `warn` or `error`.
export type RecordLevelFilter = "attention" | RecordLevel;

export const RECORD_LEVEL_FILTERS: readonly RecordLevelFilter[] = ["attention", ...RECORD_LEVELS];

// Where the next page starts: the last summary of the page before it.
export interface RecordCursor {
  time: string;
  kind: RecordKind;
  id: number;
}

export interface RecordsQuery {
  // A launch, named by its session.
  session: string | null;
  kind: RecordKind | null;
  level: RecordLevelFilter | null;
  search: string;
  after: RecordCursor | null;
}

export interface RecordSummary {
  kind: RecordKind;
  id: number;
  session: string | null;
  time: string;
  level: RecordLevel;
  title: string;
  text: string | null;
  // An Issue's or a notification's sentence, as the key and the JSON values
  // it was recorded with.
  messageKey: string | null;
  messageValues: string | null;
}

export interface RecordsPage {
  records: RecordSummary[];
  more: boolean;
}

// One stored column, as the database holds it.
export interface RecordField {
  name: string;
  value: string | number | null;
}

export interface RecordDetail extends RecordSummary {
  // Every column of the row except its id, in table order.
  fields: RecordField[];
}

export interface RecordSources {
  currentSession: string | null;
  // Every launch that has records, newest first.
  sessions: string[];
}

export function recordKey(record: { kind: RecordKind; id: number }): string {
  return `${record.kind}:${record.id}`;
}

// Stored JSON, indented for reading; text that is not JSON is shown as it is.
export function prettyJson(text: string): string {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

export const KIND_LABELS: Record<RecordKind, MessageKey> = {
  log: "records.kindLog",
  activity: "records.kindActivity",
  trash: "records.kindDeletedFiles",
  analysis: "records.kindAnalysis",
  issue: "records.kindIssue",
  notice: "records.kindNotice",
};

export const LEVEL_LABELS: Record<RecordLevel, MessageKey> = {
  error: "records.levelError",
  warn: "records.levelWarn",
  info: "records.levelInfo",
  debug: "records.levelDebug",
};

export const LEVEL_FILTER_LABELS: Record<RecordLevelFilter, MessageKey> = {
  attention: "records.levelAttention",
  ...LEVEL_LABELS,
};

export const LEVEL_TONES: Record<RecordLevel, string> = {
  error: "bg-danger-surface text-danger",
  warn: "bg-warning-surface text-warning",
  info: "bg-surface-muted text-ink-muted",
  debug: "bg-surface-muted text-ink-muted",
};

// How a stored column is shown: as a stored value or code, plain text, the
// recorded sentence in the interface language, a time, a duration in milliseconds, the launch it came from, or a block of
// stored JSON.
export type FieldShape = "value" | "text" | "sentence" | "time" | "milliseconds" | "launch" | "json";

export interface FieldPresentation {
  label: MessageKey;
  shape: FieldShape;
}

const FIELDS: Readonly<Record<string, FieldPresentation>> = {
  session_id: { label: "records.launch", shape: "launch" },
  time_utc: { label: "records.time", shape: "time" },
  event_time_utc: { label: "records.time", shape: "time" },
  level: { label: "records.level", shape: "value" },
  message: { label: "records.message", shape: "text" },
  line: { label: "records.line", shape: "json" },
  sequence: { label: "records.sequence", shape: "value" },
  monotonic_ms: { label: "records.sinceLaunch", shape: "milliseconds" },
  operation_id: { label: "records.operation", shape: "value" },
  owner: { label: "records.owner", shape: "value" },
  draft_json: { label: "records.details", shape: "json" },
  user_visible: { label: "records.inActivityTrace", shape: "value" },
  action: { label: "records.action", shape: "value" },
  content_hash: { label: "records.contentHash", shape: "value" },
  original_path: { label: "records.originalPath", shape: "value" },
  stored_path: { label: "records.storedPath", shape: "value" },
  detail_json: { label: "records.details", shape: "json" },
  class: { label: "records.analysisClass", shape: "value" },
  event: { label: "records.event", shape: "value" },
  model: { label: "records.model", shape: "value" },
  model_version: { label: "records.modelVersion", shape: "value" },
  path: { label: "records.path", shape: "value" },
  message_key: { label: "records.shownText", shape: "sentence" },
  message_values: { label: "records.messageValues", shape: "json" },
  presentation: { label: "records.presentation", shape: "value" },
};

// An activity's `kind` is what happened to its operation; an Issue's or a
// notification's is the condition it reports.
const KIND_FIELD: Record<RecordKind, MessageKey> = {
  log: "records.kind",
  activity: "records.event",
  trash: "records.kind",
  analysis: "records.kind",
  issue: "records.condition",
  notice: "records.condition",
};

/** How one stored column is labelled and shown; null for a column this build
 * does not know, which the window shows under its own name. */
export function fieldPresentation(kind: RecordKind, name: string): FieldPresentation | null {
  if (name === "kind") return { label: KIND_FIELD[kind], shape: "value" };
  return Object.prototype.hasOwnProperty.call(FIELDS, name) ? FIELDS[name]! : null;
}

// A recorded sentence, as the key and values it was stored with; values that
// are not a JSON object are left out.
export function recordSentence(record: Pick<RecordSummary, "messageKey" | "messageValues">): Message | null {
  if (record.messageKey === null) return null;
  let values: MessageValues | undefined;
  try {
    const parsed: unknown = record.messageValues === null ? undefined : JSON.parse(record.messageValues);
    if (typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)) {
      values = Object.fromEntries(
        Object.entries(parsed).filter((entry): entry is [string, string | number] =>
          typeof entry[1] === "string" || typeof entry[1] === "number"),
      );
    }
  } catch {
    values = undefined;
  }
  return message(record.messageKey as MessageKey, values);
}

// The page after the last record shown.
export function cursorAfter(records: readonly RecordSummary[]): RecordCursor | null {
  const last = records[records.length - 1];
  return last === undefined ? null : { time: last.time, kind: last.kind, id: last.id };
}

// The order the list shows records in, newest first; the core pages them the
// same way.
export function newestFirst(a: RecordSummary, b: RecordSummary): number {
  if (a.time !== b.time) return a.time < b.time ? 1 : -1;
  if (a.kind !== b.kind) return a.kind < b.kind ? 1 : -1;
  return b.id - a.id;
}

// The newest page read again, joined with the rows already shown: a row in
// both takes the page's copy, and the rows shown beyond the page stay, so the
// pages already read are kept and a page read out of order loses nothing.
export function mergeNewestPage(
  shown: readonly RecordSummary[],
  shownMore: boolean,
  page: RecordsPage,
): { records: RecordSummary[]; more: boolean } {
  const byKey = new Map(shown.map((record) => [recordKey(record), record]));
  for (const record of page.records) byKey.set(recordKey(record), record);
  const records = [...byKey.values()].sort(newestFirst);
  const last = page.records[page.records.length - 1];
  const beyond = last !== undefined && shown.some((record) => newestFirst(record, last) > 0);
  return { records, more: beyond ? shownMore : page.more };
}
