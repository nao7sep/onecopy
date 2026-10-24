import { beforeEach, afterEach, expect, it, vi } from "vitest";
// Keep the shared IPC doubles bound to this file's controls across fresh
// application-module fixtures.
import "@tauri-apps/api/core";
import "@tauri-apps/api/event";
import type { ActiveNotificationSnapshot, NotificationRecord } from "../../src/state/notifications-store";
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
  let resolve!: (snapshot: ActiveNotificationSnapshot) => void;
  let reject!: (error: Error) => void;
  mockCommand("get_active_notifications", () => new Promise<ActiveNotificationSnapshot>((finish, fail) => {
    resolve = finish;
    reject = fail;
  }));
  const installed = notifications.installNotificationWiring();
  expect(notifications.installNotificationWiring()).toBe(installed);
  await vi.waitFor(() => expect(resolve).toBeTypeOf("function"));
  for (const event of ["published", "dismissed", "cleared", "attention"]) {
    expect(listenerCount(`notification://${event}`)).toBe(1);
  }
  return { installed, resolve, reject };
}

function snapshot(revision: number, ...active: NotificationRecord[]): ActiveNotificationSnapshot {
  return { revision, active, held: [] };
}

it("ignores a stale startup reply after a newer publication", async () => {
  const pending = await pendingSnapshot();
  fireEvent("notification://published", snapshot(3, notice(2, 3), notice(3)));
  pending.resolve(snapshot(1, notice(1), notice(2)));
  await pending.installed;
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(2, 3), notice(3)]);
});

it("keeps the newest occurrence when an earlier publication arrives during startup", async () => {
  const pending = await pendingSnapshot();
  fireEvent("notification://published", snapshot(2, notice(2, 2)));
  pending.resolve(snapshot(3, notice(2, 3)));
  await pending.installed;
  fireEvent("notification://published", snapshot(2, notice(2, 2)));
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(2, 3)]);
});

it.each(["dismissed", "cleared"])("does not resurrect a late publication after %s", async (event) => {
  const pending = await pendingSnapshot();
  pending.resolve(snapshot(0));
  await pending.installed;
  fireEvent("notification://published", snapshot(1, notice(1)));
  fireEvent(`notification://${event}`, snapshot(2));
  fireEvent("notification://published", snapshot(1, notice(1)));
  expect(notifications.useNotificationsStore.getState().active).toEqual([]);
});

it("retains both IDs when complete publications arrive in reverse order", async () => {
  const pending = await pendingSnapshot();
  pending.resolve(snapshot(0));
  await pending.installed;
  fireEvent("notification://published", snapshot(2, notice(1), notice(2)));
  fireEvent("notification://published", snapshot(1, notice(1)));
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(2), notice(1)]);
});

it("uses the dismiss reply to supersede a stale pending startup reply", async () => {
  const pending = await pendingSnapshot();
  mockCommand("dismiss_notification", () => snapshot(3, notice(2)));
  fireEvent("notification://published", snapshot(1, notice(1)));
  await notifications.useNotificationsStore.getState().dismiss(1);
  pending.resolve(snapshot(2, notice(1), notice(2)));
  await pending.installed;
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(2)]);
  expect(notifications.useNotificationsStore.getState().dismissing.size).toBe(0);
});

it("preserves a newer republish when an older dismissal reply resolves", async () => {
  const pending = await pendingSnapshot();
  pending.resolve(snapshot(1, notice(1)));
  await pending.installed;
  let finish!: (value: ActiveNotificationSnapshot) => void;
  mockCommand("dismiss_notification", () => new Promise<ActiveNotificationSnapshot>((resolve) => { finish = resolve; }));
  const dismissing = notifications.useNotificationsStore.getState().dismiss(1);
  expect(notifications.useNotificationsStore.getState().dismissing.has(1)).toBe(true);
  fireEvent("notification://published", snapshot(3, notice(1, 2)));
  finish(snapshot(2));
  await dismissing;
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(1, 2)]);
  expect(notifications.useNotificationsStore.getState().dismissing.size).toBe(0);
});

it("ignores equal revisions and cleans up a failed dismissal independently", async () => {
  const pending = await pendingSnapshot();
  pending.resolve(snapshot(1, notice(1)));
  await pending.installed;
  fireEvent("notification://cleared", snapshot(1));
  mockCommand("dismiss_notification", () => Promise.reject(new Error("dismiss failed")));
  mockCommand("publish_notification", () => notice(2));
  await notifications.useNotificationsStore.getState().dismiss(1);
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(1)]);
  expect(notifications.useNotificationsStore.getState().dismissing.size).toBe(0);
});

it("keeps normal publication, dismissal and clear after shared startup settles", async () => {
  const pending = await pendingSnapshot();
  pending.resolve(snapshot(1, notice(1)));
  await pending.installed;
  await notifications.installNotificationWiring();
  expect(invokeCalls.filter((call) => call.command === "get_active_notifications")).toHaveLength(1);
  fireEvent("notification://published", snapshot(2, notice(1), notice(2)));
  fireEvent("notification://dismissed", snapshot(3, notice(2)));
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(2)]);
  mockCommand("dismiss_notification", () => snapshot(4));
  await notifications.useNotificationsStore.getState().dismiss(2);
  expect(notifications.useNotificationsStore.getState().active).toEqual([]);
  fireEvent("notification://published", snapshot(5, notice(3)));
  fireEvent("notification://cleared", snapshot(6));
  expect(notifications.useNotificationsStore.getState().active).toEqual([]);
});

it("uses the snapshot for events before every startup listener was admitted", async () => {
  let finishRegistration!: () => void;
  listen.mockImplementationOnce(originalListen).mockImplementationOnce((event, callback) =>
    new Promise<() => void>((resolve) => {
      finishRegistration = () => { void originalListen(event, callback).then(resolve); };
    }),
  );
  mockCommand("get_active_notifications", () => snapshot(2));
  const installed = notifications.installNotificationWiring();
  await vi.waitFor(() => expect(finishRegistration).toBeTypeOf("function"));
  fireEvent("notification://published", snapshot(1, notice(7)));
  fireEvent("notification://dismissed", snapshot(2));
  finishRegistration();
  await installed;
  expect(notifications.useNotificationsStore.getState().active).toEqual([]);
});

it("disposes failed startup callbacks before installing a fresh snapshot", async () => {
  const pending = await pendingSnapshot();
  const callbacks = listen.mock.calls.map(([, callback]) => callback);
  fireEvent("notification://published", snapshot(3, notice(7)));
  pending.reject(new Error("snapshot failed"));
  await expect(pending.installed).rejects.toThrow("snapshot failed");
  for (const event of ["published", "dismissed", "cleared", "attention"]) {
    expect(listenerCount(`notification://${event}`)).toBe(0);
  }
  callbacks[0]!({ payload: snapshot(4, notice(8)) });
  callbacks[1]!({ payload: snapshot(5) });
  callbacks[2]!({ payload: snapshot(6) });
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(7)]);

  mockCommand("get_active_notifications", () => snapshot(7, notice(1)));
  await notifications.installNotificationWiring();
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(1)]);
  for (const event of ["published", "dismissed", "cleared", "attention"]) {
    expect(listenerCount(`notification://${event}`)).toBe(1);
  }
});

it("removes a partially registered startup before retrying it", async () => {
  listen.mockImplementationOnce(originalListen).mockRejectedValueOnce(new Error("registration failed"));
  await expect(notifications.installNotificationWiring()).rejects.toThrow("registration failed");
  expect(listenerCount("notification://published")).toBe(0);
  expect(invokeCalls.some((call) => call.command === "get_active_notifications")).toBe(false);
  mockCommand("get_active_notifications", () => snapshot(1, notice(1)));
  await notifications.installNotificationWiring();
  expect(notifications.useNotificationsStore.getState().active).toEqual([notice(1)]);
  for (const event of ["published", "dismissed", "cleared", "attention"]) {
    expect(listenerCount(`notification://${event}`)).toBe(1);
  }
});


it("coalesces attention changes behind one physical request and keeps newer snapshots", async () => {
  const pending = await pendingSnapshot();
  pending.resolve(snapshot(1, notice(7)));
  await pending.installed;
  const replies: Array<(value: ActiveNotificationSnapshot) => void> = [];
  mockCommand("set_notification_attention", () => new Promise<ActiveNotificationSnapshot>((resolve) => replies.push(resolve)));
  const first = Symbol(), second = Symbol();
  notifications.setNotificationAttention(first, 7, true);
  notifications.setNotificationAttention(second, 7, true);
  notifications.setNotificationAttention(first, 7, false);
  notifications.setNotificationAttention(second, 7, false);
  expect(replies).toHaveLength(1);
  expect(notifications.useNotificationsStore.getState().localHeld.size).toBe(0);
  replies[0]({ ...snapshot(2, notice(7)), held: [7] });
  await vi.waitFor(() => expect(replies).toHaveLength(2));
  expect(invokeCalls.filter(call => call.command === "set_notification_attention").map(call => call.args.ids)).toEqual([[7], []]);
  fireEvent("notification://attention", { ...snapshot(4, notice(7)), held: [7] });
  replies[1](snapshot(3, notice(7)));
  await Promise.resolve();
  expect(notifications.useNotificationsStore.getState().held.has(7)).toBe(true);
});
