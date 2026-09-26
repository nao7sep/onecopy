import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { documentTranslator } from "../i18n/I18nContext";
import { message, type Message, type MessageValues } from "../i18n/translate";
import type { MessageKey } from "../i18n/catalogues";
import { log, toErrorFields } from "../repositories";
import {
  presentEscapedFailure,
  recordInterfaceFailure,
} from "../utils/failureSurface";

export type NotificationLevel = "info" | "warning" | "error";
export type NotificationPresentation = "timed" | "persistent";

/** JSON-safe interpolation values: what actually crosses IPC and gets stored
 * beside a message key. A nested Message value (rare — only OneCopy's own
 * local last-resort path uses one) is flattened to text at record time, since
 * there is nowhere durable to keep ITS OWN key once serialized; everything
 * else round-trips exactly. */
export type StoredMessageValues = Record<string, string | number>;

function storedValues(values: MessageValues | undefined): StoredMessageValues | undefined {
  if (values === undefined) return undefined;
  const flattened: StoredMessageValues = {};
  for (const [name, value] of Object.entries(values)) {
    flattened[name] = typeof value === "object" ? documentTranslator().text(value) : value;
  }
  return flattened;
}

export interface NotificationRecord {
  id: number;
  kind: string;
  path: string | null;
  level: NotificationLevel;
  presentation: NotificationPresentation;
  message: string;
  messageKey: MessageKey | null;
  messageValues: StoredMessageValues | null;
  firstSeenUtc: string;
  lastSeenUtc: string;
  occurrenceCount: number;
}

export interface NotificationRequest {
  kind: string;
  path?: string | null;
  level: NotificationLevel;
  presentation: NotificationPresentation;
  message: string;
  messageKey?: MessageKey;
  messageValues?: StoredMessageValues;
}

/** The sentence a record's OWN condition supplies, in the current interface
 * language when it carries a descriptor; the recorded text verbatim for a row
 * from before this descriptor existed (interface-language.md L12/L13), or a
 * generic sentence for a condition that never named one. Real, unrestatable
 * detail (a system error, a path) is not this function's job — it lives in
 * `record.message` as recorded, shown alongside this sentence, exactly as it
 * did before a descriptor existed for any row. */
export function noticeSentence(
  record: { messageKey: MessageKey | null; messageValues: StoredMessageValues | null; message: string | null },
  text: (message: Message) => string,
): string {
  if (record.messageKey != null) {
    return text({ key: record.messageKey, values: record.messageValues ?? undefined });
  }
  return record.message !== null && record.message.trim() !== ""
    ? record.message
    : text(message("notice.backgroundStopped"));
}

/** A live Message's descriptor, ready to spread into a request. */
function descriptorFields(failure: Message): { messageKey: MessageKey; messageValues?: StoredMessageValues } {
  return { messageKey: failure.key, messageValues: storedValues(failure.values) };
}

interface NotificationsState {
  active: NotificationRecord[];
  dismissing: Set<number>;
  dismiss: (id: number) => Promise<void>;
}

function newestFirst(rows: NotificationRecord[]): NotificationRecord[] {
  return [...rows].sort(
    (left, right) =>
      right.lastSeenUtc.localeCompare(left.lastSeenUtc) || right.id - left.id,
  );
}

function mergeRecord(
  rows: NotificationRecord[],
  record: NotificationRecord,
): NotificationRecord[] {
  return newestFirst([...rows.filter((row) => row.id !== record.id), record]);
}

export const useNotificationsStore = create<NotificationsState>((set) => ({
  active: [],
  dismissing: new Set(),
  dismiss: async (id) => {
    set((state) => ({ dismissing: new Set(state.dismissing).add(id) }));
    try {
      await invoke("dismiss_notification", { id });
      set((state) => ({
        active: state.active.filter((row) => row.id !== id),
        dismissing: new Set([...state.dismissing].filter((value) => value !== id)),
      }));
    } catch (error) {
      log.error("notification dismissal failed", toErrorFields(error));
      set((state) => ({
        dismissing: new Set([...state.dismissing].filter((value) => value !== id)),
      }));
      reportActionFailure(
        "notification-dismiss-failed",
        message("notice.dismissFailed"),
        error,
      );
    }
  },
}));

let installation: Promise<void> | null = null;

async function install(): Promise<void> {
  const unlisten: Array<() => void> = [];
  try {
    unlisten.push(await listen<NotificationRecord>("notification://published", (event) => {
      useNotificationsStore.setState((state) => ({
        active: mergeRecord(state.active, event.payload),
      }));
    }));
    unlisten.push(await listen<{ id: number }>("notification://dismissed", (event) => {
      useNotificationsStore.setState((state) => ({
        active: state.active.filter((row) => row.id !== event.payload.id),
        dismissing: new Set(
          [...state.dismissing].filter((value) => value !== event.payload.id),
        ),
      }));
    }));
    unlisten.push(await listen("notification://cleared", () => {
      useNotificationsStore.setState({ active: [], dismissing: new Set() });
    }));
    const current = await invoke<NotificationRecord[]>("get_active_notifications");
    useNotificationsStore.setState((state) => ({
      active: newestFirst(
        current.reduce(
          (rows, record) => mergeRecord(rows, record),
          state.active,
        ),
      ),
    }));
  } catch (error) {
    for (const stop of unlisten) stop();
    throw error;
  }
}

export function installNotificationWiring(): Promise<void> {
  installation ??= install().catch((error) => {
    installation = null;
    log.error("notification event wiring failed", toErrorFields(error));
    throw error;
  });
  return installation;
}

export async function publishNotification(
  request: NotificationRequest,
): Promise<NotificationRecord> {
  return invoke<NotificationRecord>("publish_notification", { request });
}

export async function recordRecentNotification(
  request: NotificationRequest,
): Promise<NotificationRecord> {
  return invoke<NotificationRecord>("record_recent_notification", { request });
}

export async function errorNotification(
  kind: string,
  failure: Message,
  error?: unknown,
): Promise<NotificationRecord> {
  if (error !== undefined) {
    log.error("notification action failed", { kind, ...toErrorFields(error) });
  }
  // The message descriptor (key + values) crosses IPC and is stored beside
  // the record, so every window renders it in ITS current interface
  // language — including after a later language change, not just the one
  // active in this window when the failure happened (R5.5 D-L12).
  return publishNotification({
    kind,
    level: "error",
    presentation: "persistent",
    message: "",
    ...descriptorFields(failure),
  });
}

/** Shows one timed information notice for an action's outcome. */
export function reportInfoNotice(kind: string, notice: Message): void {
  void publishNotification({
    kind,
    level: "info",
    presentation: "timed",
    message: "",
    ...descriptorFields(notice),
  }).catch((error) => {
    log.error("information notice failed", { kind, ...toErrorFields(error) });
  });
}

/** Records one failed user-requested action without making every caller own
 * notification persistence failure or invent a second visible error path. */
export function reportActionFailure(
  kind: string,
  failure: Message,
  error?: unknown,
): void {
  void errorNotification(kind, failure, error).catch((recordingError) => {
    handleActionFailureRecordingError(kind, failure, error, recordingError);
  });
}

/** Records a failed locally owned action in Issues and diagnostic history without
 * publishing a second live persistent notice over its inline result. */
export function recordActionFailure(
  kind: string,
  failure: Message,
  error?: unknown,
): void {
  void recordRecentNotification({
    kind,
    level: "error",
    presentation: "persistent",
    message: "",
    ...descriptorFields(failure),
  }).catch((recordingError) => {
    handleActionFailureRecordingError(kind, failure, error, recordingError);
  });
}

function handleActionFailureRecordingError(
  kind: string,
  failure: Message,
  error: unknown,
  recordingError: unknown,
): void {
  log.error("action failure notification could not be recorded", {
    kind,
    actionError: toErrorFields(error).error,
    recordingError: toErrorFields(recordingError).error,
  });
  const direct = message("notice.notSaved", { failure });
  presentEscapedFailure(direct);
  recordInterfaceFailure(direct);
}
