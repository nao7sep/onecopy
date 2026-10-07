// @vitest-environment happy-dom
import { t } from "../helpers/i18n";
import { expect, it, vi } from "vitest";
import { backgroundRows, backgroundRuntimeLine, installDerivedWorkEventWiring, useDerivedWorkStore } from "../../src/state/derived-work-store";
import { fireEvent, invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

it("reads totals only while open, coalesces invalidations, and rejects stale reads after close", async () => {
  vi.useFakeTimers();
  resetTauriMocks();
  const row = { id: "previews", state: "up-to-date", queued: 0, failed: 0, done: null, total: null, reason: null };
  mockCommands({ background_work_runtime: () => ({ workerRunning: true, pausedClasses: [], active: null }), background_work_snapshot: () => ({ workerRunning: true, pausedClasses: [], classes: [row], activeItem: null }) });
  await installDerivedWorkEventWiring();
  await useDerivedWorkStore.getState().load();
  for (let index = 0; index < 100; index++) fireEvent("file-information://progress", {});
  await vi.advanceTimersByTimeAsync(1000);
  expect(invokeCalls.filter((call) => call.command === "background_work_snapshot")).toHaveLength(0);
  useDerivedWorkStore.getState().setDetailsOpen(true);
  await vi.advanceTimersByTimeAsync(0);
  let finish = (_value: unknown): void => {};
  mockCommands({ background_work_snapshot: () => new Promise((resolve) => { finish = resolve; }) });
  for (let index = 0; index < 100; index++) fireEvent("file-information://progress", {});
  await vi.advanceTimersByTimeAsync(1000);
  expect(invokeCalls.filter((call) => call.command === "background_work_snapshot")).toHaveLength(2);
  fireEvent("derived://state-changed", {
    workerRunning: true, pausedClasses: [],
    active: { id: "previews", hash: "now", done: 1, total: 20, stopping: false },
  });
  finish({ workerRunning: true, pausedClasses: [], classes: [{ ...row, state: "queued", queued: 20 }], activeItem: null });
  await vi.advanceTimersByTimeAsync(0);
  expect(backgroundRows(useDerivedWorkStore.getState().snapshot!)[0]).toMatchObject({ state: "running", queued: 20, done: 1 });
  expect(useDerivedWorkStore.getState().activeItem?.hash).toBe("now");
  const reload = useDerivedWorkStore.getState().load();
  fireEvent("derived://state-changed", { workerRunning: false, pausedClasses: [], active: null });
  finish({ workerRunning: true, pausedClasses: [], classes: [row], activeItem: null });
  await reload;
  expect(useDerivedWorkStore.getState().snapshot?.workerRunning).toBe(false);
  expect(useDerivedWorkStore.getState().activeItem).toBeNull();
  const stale = useDerivedWorkStore.getState().load();
  useDerivedWorkStore.getState().setDetailsOpen(false);
  finish({ workerRunning: true, pausedClasses: [], classes: [row], activeItem: null });
  await stale;
  expect(useDerivedWorkStore.getState().snapshot).toBeNull();
  fireEvent("derived://state-changed", { workerRunning: true, pausedClasses: [], active: { id: "faces", hash: "x", done: 2, total: 5, stopping: false } });
  expect(backgroundRuntimeLine(useDerivedWorkStore.getState().runtime, t)).toBe("Face scoring 2/5");
  const count = invokeCalls.filter((call) => call.command === "background_work_snapshot").length;
  fireEvent("derived://quiet", {});
  await vi.advanceTimersByTimeAsync(2000);
  expect(invokeCalls.filter((call) => call.command === "background_work_snapshot")).toHaveLength(count);
  vi.useRealTimers();
});
