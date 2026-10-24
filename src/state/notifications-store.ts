import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { message, type Message } from "../i18n/translate";
import { storedValues, type StoredMessageValues } from "../i18n/storedValues";
import type { MessageKey } from "../i18n/catalogues";
import { log, toErrorFields } from "../repositories";
import {
  presentEscapedFailure,
  recordInterfaceFailure,
} from "../utils/failureSurface";

export type NotificationLevel = "info" | "warning" | "error";
export type NotificationPresentation = "timed" | "persistent";
export type { StoredMessageValues };

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

export interface ActiveNotificationSnapshot {
  revision: number;
  active: NotificationRecord[];
  held: number[];
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
 * from before this descriptor existed, or a
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
  revision: number;
  active: NotificationRecord[];
  held: Set<number>;
  localHeld: Set<number>;
  dismissing: Set<number>;
  dismiss: (id: number, automatic?: boolean) => Promise<void>;
}

function newestFirst(rows: NotificationRecord[]): NotificationRecord[] {
  return [...rows].sort(
    (left, right) =>
      right.lastSeenUtc.localeCompare(left.lastSeenUtc) || right.id - left.id,
  );
}

function applySnapshot(snapshot: ActiveNotificationSnapshot): void {
  useNotificationsStore.setState((state) => snapshot.revision > state.revision
    ? { revision: snapshot.revision, active: newestFirst(snapshot.active), held: new Set(snapshot.held) }
    : state);
}

export const useNotificationsStore = create<NotificationsState>((set) => ({
  revision: -1,
  active: [],
  held: new Set(),
  localHeld: new Set(),
  dismissing: new Set(),
  dismiss: async (id, automatic = false) => {
    set((state) => ({ dismissing: new Set(state.dismissing).add(id) }));
    try {
      applySnapshot(await invoke<ActiveNotificationSnapshot>("dismiss_notification", { id, automatic }));
    } catch (error) {
      log.error("notification dismissal failed", toErrorFields(error));
      reportActionFailure(
        "notification-dismiss-failed",
        message("notice.dismissFailed"),
        error,
      );
    } finally {
      set((state) => ({
        dismissing: new Set([...state.dismissing].filter((value) => value !== id)),
      }));
    }
  },
}));

// A webview publishes its complete current hold set, with one physical IPC
// request at a time. Rapid pointer/focus changes replace pending intent.
const attentionOwners = new Map<symbol, number>();
let attentionDirty = false;
let attentionWriter: Promise<void> | null = null;

export function setNotificationAttention(owner: symbol, id: number, held: boolean): void {
  if (held ? attentionOwners.get(owner) === id : !attentionOwners.has(owner)) return;
  if (held) attentionOwners.set(owner, id);
  else attentionOwners.delete(owner);
  useNotificationsStore.setState({ localHeld: new Set(attentionOwners.values()) });
  attentionDirty = true;
  flushAttention();
}

function flushAttention(): void {
  if (attentionWriter) return;
  attentionWriter = (async () => {
    while (attentionDirty) {
      attentionDirty = false;
      const ids = [...new Set(attentionOwners.values())];
      try {
        applySnapshot(await invoke<ActiveNotificationSnapshot>("set_notification_attention", { ids }));
      } catch (error) {
        log.error("notification attention update failed", toErrorFields(error));
        recordInterfaceFailure(message("notice.unavailable"), error);
      }
    }
  })().finally(() => {
    attentionWriter = null;
    if (attentionDirty) flushAttention();
  });
}

let installation: Promise<void> | null = null;

async function install(): Promise<void> {
  const unlisten: Array<() => void> = [];
  let disposed = false;
  try {
    for (const event of ["published", "dismissed", "cleared", "attention"]) {
      unlisten.push(await listen<ActiveNotificationSnapshot>(`notification://${event}`, (event) => {
        if (!disposed) applySnapshot(event.payload);
      }));
    }
    applySnapshot(await invoke<ActiveNotificationSnapshot>("get_active_notifications"));
  } catch (error) {
    disposed = true;
    for (const stop of unlisten.reverse()) {
      try { stop(); } catch { /* Retire every registered listener, even after one failed removal. */ }
    }
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
  // active in this window when the failure happened.
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
  recordInterfaceFailure(direct, recordingError);
}
