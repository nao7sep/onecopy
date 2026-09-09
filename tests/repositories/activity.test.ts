import { expect, it } from "vitest";
import { recordActivity } from "../../src/repositories/activity";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

it("keeps lifecycle publication ordered without blocking the caller and continues after recording failure", async () => {
  resetTauriMocks();
  let reject!: (error: Error) => void;
  mockCommands({ activity_record: ({ draft }) => (draft as { kind: string }).kind === "started"
    ? new Promise((_, fail) => { reject = fail; }) : null });
  recordActivity({ owner: "settings", kind: "started", operationId: "settings:one" });
  recordActivity({ owner: "settings", kind: "completed", operationId: "settings:one" });
  await Promise.resolve();
  expect(invokeCalls.filter((call) => call.command === "activity_record")).toHaveLength(1);
  reject(new Error("diagnostic write failed"));
  for (let index = 0; index < 10; index++) await Promise.resolve();
  expect(invokeCalls.filter((call) => call.command === "activity_record").map((call) => (call.args.draft as { kind: string }).kind)).toEqual(["started", "completed"]);
});
