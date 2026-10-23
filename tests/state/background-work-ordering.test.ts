// @vitest-environment happy-dom
import { expect, it } from "vitest";
import {
  backgroundRows,
  installDerivedWorkEventWiring,
  type BackgroundRuntimeSnapshot,
  type BackgroundWorkSnapshot,
  useDerivedWorkStore,
} from "../../src/state/derived-work-store";
import { fireEvent, invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

it("orders startup replies, live events, and detailed runtime by backend capture revision", async () => {
  resetTauriMocks();
  useDerivedWorkStore.setState({ runtime: null, snapshot: null, activeItem: null, detailsOpen: false, loading: false });
  let finishInitial!: (runtime: BackgroundRuntimeSnapshot) => void;
  const initial = new Promise<BackgroundRuntimeSnapshot>((resolve) => { finishInitial = resolve; });
  const active: BackgroundRuntimeSnapshot = {
    revision: 2,
    workerRunning: true,
    pausedClasses: [],
    active: { id: "previews", hash: "photo", done: 2, total: 8, stopping: false },
  };
  const detail: BackgroundWorkSnapshot = {
    revision: 1,
    workerRunning: false,
    pausedClasses: [],
    activeItem: null,
    classes: [{ id: "previews", state: "queued", queued: 8, failed: 1, done: null, total: null, reason: null }],
  };
  mockCommands({ background_work_runtime: () => initial, background_work_snapshot: () => detail });
  const wiring = installDerivedWorkEventWiring();
  // The startup read must already be in flight when the newer event arrives.
  for (let turn = 0; turn < 30 && !invokeCalls.some((call) => call.command === "background_work_runtime"); turn++) {
    await Promise.resolve();
  }
  expect(invokeCalls.some((call) => call.command === "background_work_runtime")).toBe(true);
  fireEvent("derived://state-changed", active);
  finishInitial({ revision: 1, workerRunning: false, pausedClasses: [], active: null });
  await wiring;
  expect(useDerivedWorkStore.getState().runtime).toEqual(active);
  expect(useDerivedWorkStore.getState().activeItem?.hash).toBe("photo");

  // Even a read requested after an event can return an older capture.
  useDerivedWorkStore.getState().setDetailsOpen(true);
  await useDerivedWorkStore.getState().load();
  expect(backgroundRows(useDerivedWorkStore.getState().snapshot!)[0]).toMatchObject({
    state: "running", done: 2, total: 8, queued: 8, failed: 1,
  });
  expect(useDerivedWorkStore.getState().snapshot?.workerRunning).toBe(true);

  let finishDetail!: (snapshot: BackgroundWorkSnapshot) => void;
  mockCommands({ background_work_snapshot: () => new Promise<BackgroundWorkSnapshot>((resolve) => { finishDetail = resolve; }) });
  const refresh = useDerivedWorkStore.getState().load();
  fireEvent("derived://state-changed", { ...active, revision: 3, active: { ...active.active!, done: 5 } });
  // Captured after the event's state, although delivered on the IPC channel.
  finishDetail({ ...detail, revision: 4, workerRunning: true, classes: [{ ...detail.classes[0], state: "up-to-date", queued: 0 }] });
  await refresh;
  expect(useDerivedWorkStore.getState().runtime).toMatchObject({ revision: 4, active: null });
  expect(useDerivedWorkStore.getState().activeItem).toBeNull();
  expect(backgroundRows(useDerivedWorkStore.getState().snapshot!)[0]).toMatchObject({ state: "up-to-date", queued: 0, failed: 1 });

  // Delayed progress and an equal-revision duplicate cannot revive finished work.
  fireEvent("derived://state-changed", { ...active, revision: 3 });
  fireEvent("derived://state-changed", { ...active, revision: 4 });
  expect(useDerivedWorkStore.getState().runtime).toMatchObject({ revision: 4, active: null });
  expect(useDerivedWorkStore.getState().activeItem).toBeNull();
  expect(useDerivedWorkStore.getState().snapshot?.activeItem).toBeNull();
  expect(backgroundRows(useDerivedWorkStore.getState().snapshot!)[0]).toMatchObject({ state: "up-to-date", queued: 0, failed: 1, done: null });
  useDerivedWorkStore.getState().setDetailsOpen(false);
});
