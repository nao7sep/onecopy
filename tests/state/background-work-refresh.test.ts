// @vitest-environment happy-dom
import { expect, it, vi } from "vitest";
import { installDerivedWorkEventWiring, useDerivedWorkStore } from "../../src/state/derived-work-store";
import { fireEvent, invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

it("refreshes newly discovered debt without an open modal and preserves newer running state", async () => {
  vi.useFakeTimers();
  resetTauriMocks();
  const row = { id: "previews", state: "up-to-date", queued: 0, failed: 0, done: null, total: null, reason: null };
  mockCommands({ background_work_snapshot: () => ({ masterPaused: false, classes: [row], activeItem: null }) });
  await installDerivedWorkEventWiring();
  let finish = (_value: unknown): void => {};
  mockCommands({ background_work_snapshot: () => new Promise((resolve) => { finish = resolve; }) });
  for (let index = 0; index < 100; index++) fireEvent("file-information://progress", {});
  await vi.advanceTimersByTimeAsync(1000);
  expect(invokeCalls.filter((call) => call.command === "background_work_snapshot")).toHaveLength(2);
  fireEvent("derived://state-changed", {
    masterPaused: false, pausedClasses: [],
    active: { id: "previews", hash: "now", done: 1, total: 20, stopping: false },
  });
  finish({ masterPaused: false, classes: [{ ...row, state: "queued", queued: 20 }], activeItem: null });
  await vi.advanceTimersByTimeAsync(0);
  expect(useDerivedWorkStore.getState().snapshot?.classes[0]).toMatchObject({ state: "running", queued: 20, done: 1 });
  expect(useDerivedWorkStore.getState().activeItem?.hash).toBe("now");
  vi.useRealTimers();
});
