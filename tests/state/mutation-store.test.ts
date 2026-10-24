// @vitest-environment happy-dom
//
// Failures are recorded through the notifications store, which renders the
// sentence in the language the document declares, so this spec needs a document
// even though the subject is not the interface.

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MutationOutcome, MutationProgress, MutationResultSummary } from "../../src/models/mutation";
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
  useMutationStore.setState({
    progress: null,
    cancelling: false,
    result: null,
    exiting: false,
  });
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

    fireEvent("mutation://done", {
      progress: { ...PROGRESS, phase: "complete" },
      cancelled: true,
      outcome: "cancelled",
      summary: {
        itemsCompleted: 1,
        itemsPartial: 1,
        itemsUnstarted: 3,
        filesCompleted: 2,
        filesFailed: 0,
        filesUnknown: 0,
        filesUnstarted: 6,
        trashAvailable: true,
        error: null,
      },
    });
    expect(useMutationStore.getState()).toMatchObject({
      progress: null,
      cancelling: false,
      result: { cancelled: true },
    });
  });

  it("projects the exit quiescence wait without offering a force path", async () => {
    await installMutationEventWiring();
    fireEvent("app://exit-quiescing", null);

    expect(useMutationStore.getState()).toMatchObject({
      exiting: true,
      cancelling: true,
    });
  });

  it("records a failed final result in restart-persistent Recent history", async () => {
    mockCommands({
      record_recent_notification: () => ({
        id: 1,
        kind: "delete-failed",
        path: null,
        level: "warning",
        presentation: "persistent",
        message: "Delete stopped: 1 file failed.",
        firstSeenUtc: "2026-08-31T00:00:00.000Z",
        lastSeenUtc: "2026-08-31T00:00:00.000Z",
        occurrenceCount: 1,
      }),
    });
    await installMutationEventWiring();
    fireEvent("mutation://progress", PROGRESS);
    fireEvent("mutation://done", {
      progress: { ...PROGRESS, phase: "complete" },
      cancelled: false,
      outcome: "failed",
      summary: {
        itemsCompleted: 1,
        itemsPartial: 1,
        itemsUnstarted: 3,
        filesCompleted: 2,
        filesFailed: 1,
        filesUnknown: 0,
        filesUnstarted: 5,
        trashAvailable: true,
        error: null,
      },
    });

    await vi.waitFor(() => {
      expect(invokeCalls.some((call) => call.command === "record_recent_notification")).toBe(true);
    });
    const request = invokeCalls.find(
      (call) => call.command === "record_recent_notification",
    )?.args.request as { kind?: string; level?: string } | undefined;
    expect(request).toMatchObject({ kind: "delete-failed", level: "warning" });
  });

  it("keeps a cancellation command failure visible", async () => {
    mockCommands({ mutation_cancel: () => Promise.reject(new Error("runtime unavailable")) });
    useMutationStore.setState({ progress: PROGRESS });

    await useMutationStore.getState().cancel();

    expect(useMutationStore.getState().cancelling).toBe(false);
    expect(invokeCalls.find((call) => call.command === "publish_notification")?.args.request).toMatchObject({
      messageKey: "mutation.cancelFailed", presentation: "persistent", level: "error",
    });
  });
});

it.each([
  ["succeeded", "completed", "completion"],
  ["failed", "failed", "error"],
  ["outcomeUnknown", "failed", "error"],
  ["cancelled", "cancelled", "user"],
  ["idle", "closed", "completion"],
] as const)("retains native %s completion in the renderer Activity", async (outcome, kind, reason) => {
  await installMutationEventWiring();
  fireEvent("mutation://progress", PROGRESS);
  fireEvent("mutation://done", {
    progress: { ...PROGRESS, phase: "complete" }, outcome,
    cancelled: outcome === "cancelled", summary: terminalSummary(outcome),
  });
  await vi.waitFor(() => expect(invokeCalls.filter(call => call.command === "activity_record")).toHaveLength(2));
  const terminal = invokeCalls.filter(call => call.command === "activity_record")
    .map(call => call.args.draft).at(-1);
  expect(terminal).toMatchObject({ kind, owner: "mutation", operationId: "mutation:42",
    previous: "running", current: outcome, reason });
});

it.each(["failed", "outcomeUnknown"] as const)("retains native %s on the error route", async (outcome) => {
  await installMutationEventWiring();
  fireEvent("mutation://progress", PROGRESS);
  fireEvent("mutation://error", { operationId: PROGRESS.operationId, kind: PROGRESS.kind,
    error: "operation stopped", outcome, summary: terminalSummary(outcome) });
  await vi.waitFor(() => expect(invokeCalls.filter(call => call.command === "activity_record")).toHaveLength(2));
  const terminal = invokeCalls.filter(call => call.command === "activity_record")
    .map(call => call.args.draft).at(-1);
  expect(terminal).toMatchObject({ kind: "failed", owner: "mutation", operationId: "mutation:42",
    previous: "running", current: outcome, reason: "error" });
});

function terminalSummary(outcome: MutationOutcome): MutationResultSummary | null {
  if (outcome === "idle") return null;
  return { itemsCompleted: 1, itemsPartial: 0, itemsUnstarted: 0, filesCompleted: 1,
    filesFailed: outcome === "failed" ? 1 : 0, filesUnknown: outcome === "outcomeUnknown" ? 1 : 0,
    filesUnstarted: 0, trashAvailable: true, error: null };
}
