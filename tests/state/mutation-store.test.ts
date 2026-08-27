import { beforeEach, describe, expect, it } from "vitest";
import type { MutationProgress } from "../../src/models/mutation";
import { useMutationStore } from "../../src/state/mutation-store";
import { installMutationEventWiring } from "../../src/workflows/mutation-events";
import { fireEvent, invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

const PROGRESS: MutationProgress = {
  operationId: 42,
  kind: "delete",
  phase: "deleting",
  itemsDone: 1,
  itemsTotal: 5,
  filesDone: 2,
  filesTotal: 8,
  bytesDone: 20,
  bytesTotal: 80,
  failures: 0,
  currentFileBytesDone: null,
  currentFileBytesTotal: null,
  nextPhase: "complete",
};

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useMutationStore.setState({ progress: null, cancelling: false });
});

describe("the shared mutation activity projection", () => {
  it("binds cancellation to the operation id and keeps it monotonic", async () => {
    mockCommands({ mutation_cancel: () => true });
    await installMutationEventWiring();
    fireEvent("mutation://progress", PROGRESS);

    await useMutationStore.getState().cancel();
    expect(invokeCalls).toContainEqual({
      command: "mutation_cancel",
      args: { operationId: 42 },
    });
    expect(useMutationStore.getState().cancelling).toBe(true);

    fireEvent("mutation://progress", { ...PROGRESS, filesDone: 3 });
    expect(useMutationStore.getState().cancelling).toBe(true);

    fireEvent("mutation://done", { progress: { ...PROGRESS, phase: "complete" }, cancelled: true });
    expect(useMutationStore.getState()).toMatchObject({
      progress: null,
      cancelling: false,
    });
  });
});
