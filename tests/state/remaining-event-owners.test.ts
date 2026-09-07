import { beforeEach, describe, expect, it } from "vitest";
import {
  emit,
  listen,
  listenerCount,
  mockCommand,
  resetTauriMocks,
} from "../mocks/tauri";

const listenNormally = listen.getMockImplementation()!;

beforeEach(() => {
  resetTauriMocks();
  listen.mockImplementation(listenNormally);
  mockCommand("log_event", () => null);
});

describe("remaining event owners", () => {
  it("rolls back a partially installed main-window owner and retries cleanly", async () => {
    const { installContentSessionWorkflow } = await import(
      "../../src/workflows/content-session"
    );
    listen
      .mockImplementationOnce(listenNormally)
      .mockRejectedValueOnce(new Error("second listener failed"));

    await expect(installContentSessionWorkflow()).resolves.toBeUndefined();
    expect(listenerCount("content-session://set-text-wrap")).toBe(0);

    await expect(installContentSessionWorkflow()).resolves.toBeUndefined();
    expect(listenerCount("content-session://set-text-wrap")).toBe(1);
    expect(listenerCount("content-session://client-ready")).toBe(1);
  });

  it("removes a client listener when its ready announcement fails", async () => {
    const { installContentSessionClient } = await import(
      "../../src/state/content-session-store"
    );
    const cause = new Error("ready delivery failed");
    emit.mockRejectedValueOnce(cause);

    await expect(installContentSessionClient()).rejects.toBe(cause);
    expect(listenerCount("content-session://state")).toBe(0);

    await expect(installContentSessionClient()).resolves.toBeUndefined();
    expect(listenerCount("content-session://state")).toBe(1);
  });

  it("removes media listeners when initial ownership reconciliation fails", async () => {
    let attempts = 0;
    const cause = new Error("media ownership unavailable");
    mockCommand("media_use_current", () => {
      attempts += 1;
      if (attempts === 1) throw cause;
      return null;
    });
    const { installMediaUseBoundary } = await import("../../src/media-use");

    await expect(installMediaUseBoundary()).rejects.toBe(cause);
    expect(listenerCount("media-use://release")).toBe(0);
    expect(listenerCount("media-use://resume")).toBe(0);

    await expect(installMediaUseBoundary()).resolves.toBeUndefined();
    expect(listenerCount("media-use://release")).toBe(1);
    expect(listenerCount("media-use://resume")).toBe(1);
  });
});
