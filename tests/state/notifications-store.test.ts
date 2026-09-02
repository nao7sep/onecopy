// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  installNotificationWiring,
  recordActionFailure,
  reportActionFailure,
  useNotificationsStore,
} from "../../src/state/notifications-store";
import {
  invokeCalls,
  listenerCount,
  mockCommands,
  resetTauriMocks,
} from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useNotificationsStore.setState({ active: [], dismissing: new Set() });
  document.getElementById("onecopy-escaped-failure")?.remove();
});

describe("notification failure containment", () => {
  it("removes partial listeners before retrying a failed installation", async () => {
    mockCommands({
      get_active_notifications: () => Promise.reject(new Error("snapshot failed")),
    });
    await expect(installNotificationWiring()).rejects.toThrow("snapshot failed");
    expect(listenerCount("notification://published")).toBe(0);
    expect(listenerCount("notification://dismissed")).toBe(0);

    mockCommands({ get_active_notifications: () => [] });
    await installNotificationWiring();
    expect(listenerCount("notification://published")).toBe(1);
    expect(listenerCount("notification://dismissed")).toBe(1);
  });

  it("uses the direct failure surface when Recent cannot save an action failure", async () => {
    mockCommands({
      publish_notification: () => Promise.reject(new Error("database unavailable")),
      record_interface_failure: () => undefined,
    });

    reportActionFailure("open-failed", "Couldn’t open the selected file.");

    await vi.waitFor(() => {
      expect(invokeCalls.some((call) => call.command === "record_interface_failure")).toBe(true);
    });
    const direct = invokeCalls.find(
      (call) => call.command === "record_interface_failure",
    );
    expect(direct?.args.message).toContain("OneCopy could not save this notice");
    expect(document.body.textContent).toContain("OneCopy needs to reload");
  });

  it("records safe operation copy in Recent without publishing raw diagnostics", async () => {
    mockCommands({
      record_recent_notification: ({ request }) => ({
        id: 1,
        ...(request as Record<string, unknown>),
      }),
    });

    recordActionFailure(
      "settings-save-failed",
      "Couldn’t save Settings.",
      new Error("TypeError EACCES /private/tmp/HOSTILE-SENTINEL IPC wrapper"),
    );

    await vi.waitFor(() => {
      expect(invokeCalls.some((call) => call.command === "record_recent_notification")).toBe(true);
    });
    expect(invokeCalls.some((call) => call.command === "publish_notification")).toBe(false);
    const recent = invokeCalls.find((call) => call.command === "record_recent_notification");
    expect(recent?.args.request).toMatchObject({
      kind: "settings-save-failed",
      level: "error",
      presentation: "persistent",
      message: "Couldn’t save Settings.",
    });
    expect(JSON.stringify(recent?.args.request)).not.toContain("HOSTILE-SENTINEL");
  });

  it("escalates only the distinct recording failure when Recent is unavailable", async () => {
    mockCommands({
      record_recent_notification: () => Promise.reject(new Error("history unavailable")),
      record_interface_failure: () => undefined,
    });

    recordActionFailure("settings-save-failed", "Couldn’t save Settings.");

    await vi.waitFor(() => {
      expect(invokeCalls.some((call) => call.command === "record_interface_failure")).toBe(true);
    });
    expect(document.body.textContent).toContain("OneCopy needs to reload");
  });
});
