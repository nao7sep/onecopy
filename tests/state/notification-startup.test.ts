import { beforeEach, afterEach, expect, it, vi } from "vitest";
// Keep the shared IPC doubles bound to this file's controls across fresh
// application-module fixtures.
import "@tauri-apps/api/core";
import "@tauri-apps/api/event";
import type { NotificationRecord } from "../../src/state/notifications-store";
import { fireEvent, invokeCalls, listen, listenerCount, mockCommand, resetTauriMocks } from "../mocks/tauri";

let notifications: typeof import("../../src/state/notifications-store");
const originalListen = listen.getMockImplementation()!;

function notice(id: number, occurrenceCount = 1): NotificationRecord {
  return {
    id, kind: `notice-${id}`, path: null, level: "warning", presentation: "persistent",
    message: `Notice ${id}`, messageKey: null, messageValues: null,
    firstSeenUtc: "2026-10-11T01:00:00.000Z",
    lastSeenUtc: `2026-10-11T01:00:0${occurrenceCount}.000Z`, occurrenceCount,
  };
}

beforeEach(async () => {
  vi.resetModules();
  resetTauriMocks();
  listen.mockImplementation(originalListen);
  mockCommand("log_event", () => null);
  notifications = await import("../../src/state/notifications-store");
});

afterEach(() => {
  listen.mockImplementation(originalListen);
  resetTauriMocks();
});

async function pendingSnapshot() {
  let resolve!: (rows: NotificationRecord[]) => void;
  let reject!: (error: Error) => void;
  mockCommand("get_active_notifications", () => new Promise<NotificationRecord[]>((finish, fail) => {
    resolve = finish;
    reject = fail;
  }));
  const installed = notifications.installNotificationWiring();
  expect(notifications.installNotificationWiring()).toBe(installed);
  await vi.waitFor(() => expect(resolve).toBeTypeOf("function"));
  for (const event of ["published", "dismissed", "cleared"]) {
    expect(listenerCount(`notification://${event}`)).toBe(1);
  }
  return { installed, resolve, reject };
}

it("replays startup publications and dismissals while retaining unseen snapshot rows", async () => {
  const snapshot = await pendingSnapshot();
  fireEvent("notification://published", notice(2, 3));
  fireEvent("notification://published", notice(9));
  fireEvent("notification://dismissed", { id: 1 });
  fireEvent("notification://dismissed", { id: 4 });
  fireEvent("notification://published", notice(4, 2));
  fireEvent("notification://published", notice(5));
  fireEvent("notification://dismissed", { id: 5 });
  expect(notifications.useNotificationsStore.getState().active).toContainEqual(notice(2, 3));
  snapshot.resolve([notice(1), notice(2), notice(3), notice(4), notice(5)]);
  await snapshot.installed;

  expect(notifications.useNotificationsStore.getState().active).toEqual([
    notice(2, 3), notice(4, 2), notice(9), notice(3),
  ]);
});

it("keeps a newer snapshot occurrence when a prior publication arrives during its read", async () => {
  const snapshot = await pendingSnapshot();
  fireEvent("notification://published", notice(2, 2));
  snapshot.resolve([notice(2, 3)]);
  await snapshot.installed;
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(2, 3)]);
});

it("replays clear as a boundary and retains only subsequent live changes", async () => {
  const snapshot = await pendingSnapshot();
  fireEvent("notification://published", notice(3));
  fireEvent("notification://dismissed", { id: 1 });
  fireEvent("notification://cleared");
  fireEvent("notification://published", notice(4, 2));
  fireEvent("notification://published", notice(5));
  fireEvent("notification://dismissed", { id: 5 });
  fireEvent("notification://dismissed", { id: 6 });
  fireEvent("notification://published", notice(6));
  snapshot.resolve([notice(1), notice(2)]);
  await snapshot.installed;
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(4, 2), notice(6)]);
});

it("does not resurrect an acknowledged local dismissal from the pending snapshot", async () => {
  const snapshot = await pendingSnapshot();
  mockCommand("dismiss_notification", () => true);
  fireEvent("notification://published", notice(1));
  await notifications.useNotificationsStore.getState().dismiss(1);
  snapshot.resolve([notice(1), notice(2)]);
  await snapshot.installed;
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(2)]);
  expect(notifications.useNotificationsStore.getState().dismissing.size).toBe(0);
});

it("keeps normal live publication, dismissal and clear after the shared startup settles", async () => {
  const snapshot = await pendingSnapshot();
  snapshot.resolve([notice(1)]);
  await snapshot.installed;
  await notifications.installNotificationWiring();
  expect(invokeCalls.filter((call) => call.command === "get_active_notifications")).toHaveLength(1);
  fireEvent("notification://published", notice(2));
  fireEvent("notification://dismissed", { id: 1 });
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(2)]);
  mockCommand("dismiss_notification", () => true);
  await notifications.useNotificationsStore.getState().dismiss(2);
  expect(notifications.useNotificationsStore.getState().active).toEqual([]);
  fireEvent("notification://published", notice(3));
  fireEvent("notification://cleared");
  expect(notifications.useNotificationsStore.getState().active).toEqual([]);
});

it("lets the snapshot replace events seen before every startup listener was admitted", async () => {
  let finishRegistration!: () => void;
  listen.mockImplementationOnce(originalListen).mockImplementationOnce((event, callback) =>
    new Promise<() => void>((resolve) => {
      finishRegistration = () => { void originalListen(event, callback).then(resolve); };
    }),
  );
  mockCommand("get_active_notifications", () => []);
  const installed = notifications.installNotificationWiring();
  await vi.waitFor(() => expect(finishRegistration).toBeTypeOf("function"));
  fireEvent("notification://published", notice(7));
  // Dismissal precedes that listener's admission, but the ensuing snapshot
  // already reflects it. The earlier publication cannot override that read.
  fireEvent("notification://dismissed", { id: 7 });
  finishRegistration();
  await installed;
  expect(notifications.useNotificationsStore.getState().active).toEqual([]);
});

it("disposes a failed startup's callbacks and replay before installing a fresh snapshot", async () => {
  const snapshot = await pendingSnapshot();
  const callbacks = listen.mock.calls.map(([, callback]) => callback);
  fireEvent("notification://dismissed", { id: 1 });
  fireEvent("notification://cleared");
  fireEvent("notification://published", notice(7));
  snapshot.reject(new Error("snapshot failed"));
  await expect(snapshot.installed).rejects.toThrow("snapshot failed");
  for (const event of ["published", "dismissed", "cleared"]) {
    expect(listenerCount(`notification://${event}`)).toBe(0);
  }
  callbacks[0]!({ payload: notice(8) });
  callbacks[1]!({ payload: { id: 7 } });
  callbacks[2]!({ payload: undefined });
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(7)]);

  mockCommand("get_active_notifications", () => [notice(1)]);
  await notifications.installNotificationWiring();
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(1)]);
  for (const event of ["published", "dismissed", "cleared"]) {
    expect(listenerCount(`notification://${event}`)).toBe(1);
  }
});

it("removes a partially registered startup before retrying it", async () => {
  listen.mockImplementationOnce(originalListen).mockRejectedValueOnce(new Error("registration failed"));
  await expect(notifications.installNotificationWiring()).rejects.toThrow("registration failed");
  expect(listenerCount("notification://published")).toBe(0);
  expect(invokeCalls.some((call) => call.command === "get_active_notifications")).toBe(false);
  mockCommand("get_active_notifications", () => [notice(1)]);
  await notifications.installNotificationWiring();
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(1)]);
  for (const event of ["published", "dismissed", "cleared"]) {
    expect(listenerCount(`notification://${event}`)).toBe(1);
  }
});
