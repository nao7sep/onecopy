import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
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
        "Couldn’t dismiss the notification.",
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

export function errorNotification(
  kind: string,
  message: string,
  error?: unknown,
): Promise<NotificationRecord> {
  const reason = error instanceof Error ? error.message : error == null ? "" : String(error);
  const detail = reason !== "" && !message.includes(reason) ? `${message} ${reason}` : message;
  return publishNotification({
    kind,
    level: "error",
    presentation: "persistent",
    message: detail,
  });
}

/** Records one failed user-requested action without making every caller own
 * notification persistence failure or invent a second visible error path. */
export function reportActionFailure(
  kind: string,
  message: string,
  error?: unknown,
): void {
  void errorNotification(kind, message, error).catch((recordingError) => {
    handleActionFailureRecordingError(kind, message, error, recordingError);
  });
}

/** Records a failed modal-owned action in required Recent history without
 * publishing a second live persistent notice over the modal's inline error. */
export function recordActionFailure(
  kind: string,
  message: string,
  error?: unknown,
): void {
  const reason = error instanceof Error ? error.message : error == null ? "" : String(error);
  const detail = reason !== "" && !message.includes(reason) ? `${message} ${reason}` : message;
  void recordRecentNotification({
    kind,
    level: "error",
    presentation: "persistent",
    message: detail,
  }).catch((recordingError) => {
    handleActionFailureRecordingError(kind, message, error, recordingError);
  });
}

function handleActionFailureRecordingError(
  kind: string,
  message: string,
  error: unknown,
  recordingError: unknown,
): void {
  log.error("action failure notification could not be recorded", {
    kind,
    actionError: toErrorFields(error).error,
    recordingError: toErrorFields(recordingError).error,
  });
  const reason = recordingError instanceof Error
    ? recordingError.message
    : String(recordingError);
  const direct = `${message} OneCopy could not save this notice: ${reason}`;
  presentEscapedFailure(direct);
  recordInterfaceFailure(direct);
}
