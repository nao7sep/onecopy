import { beforeEach, expect, it } from "vitest";
import { invokeCalls, listen, listenerCount, mockCommands, resetTauriMocks } from "../mocks/tauri";

const listenNormally = listen.getMockImplementation()!;
beforeEach(() => {
  resetTauriMocks();
  listen.mockImplementation(listenNormally);
  mockCommands({ log_event: () => null, quit_workflow_ready: () => null });
});

it("rolls back partial listeners and failed readiness before retrying admission", async () => {
  const { installQuitWorkflow } = await import("../../src/workflows/quit");
  const cause = new Error("quit installation unavailable");
  listen.mockImplementationOnce(listenNormally).mockRejectedValueOnce(cause);
  await expect(installQuitWorkflow()).rejects.toBe(cause);
  expect(invokeCalls.some((call) => call.command === "quit_workflow_ready")).toBe(false);
  expect(listenerCount("app://quit-request")).toBe(0);

  mockCommands({ quit_workflow_ready: () => { throw cause; } });
  await expect(installQuitWorkflow()).rejects.toBe(cause);
  for (const name of ["app://quit-request", "app://session-ending", "app://session-end-cancelled"]) {
    expect(listenerCount(name)).toBe(0);
  }
  mockCommands({ quit_workflow_ready: () => {
    for (const name of ["app://quit-request", "app://session-ending", "app://session-end-cancelled"]) {
      expect(listenerCount(name)).toBe(1);
    }
    return null;
  } });
  await expect(installQuitWorkflow()).resolves.toBeUndefined();
  await installQuitWorkflow();
  expect(listenerCount("app://quit-request")).toBe(1);
});
