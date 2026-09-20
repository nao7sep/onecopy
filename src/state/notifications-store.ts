import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { documentTranslator } from "../i18n/I18nContext";
import { message, type Message } from "../i18n/translate";
import { log, toErrorFields } from "../repositories";
import {
  presentEscapedFailure,
  recordInterfaceFailure,
} from "../utils/failureSurface";

export type NotificationLevel = "info" | "warning" | "error";
export type NotificationPresentation = "timed" | "persistent";

export interface NotificationRecord {
  id: number;
  kind: string;
  path: string | null;
  level: NotificationLevel;
  presentation: NotificationPresentation;
  message: string;
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
}

// What this window said when it raised a notice, by record id. The record that
// crosses IPC carries rendered text for history and for merging repeats; this
// keeps the descriptor so the live notice follows a language change.
const raisedHere = new Map<number, Message>();

export function noticeRaisedHere(id: number): Message | undefined {
  return raisedHere.get(id);
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
  const record = await publishNotification({
    kind,
    level: "error",
    presentation: "persistent",
    // The record crosses IPC as text, so the sentence is rendered here in the
    // language this window is showing; the descriptor stays beside it.
    message: documentTranslator().text(failure),
  });
  raisedHere.set(record.id, failure);
  return record;
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
    message: documentTranslator().text(failure),
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
