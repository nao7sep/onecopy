// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  installNotificationWiring,
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
});
