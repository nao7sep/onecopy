import { beforeEach, expect, it, vi } from "vitest";
import type { ActivityDraft } from "../../src/repositories/activity";
import { ensureRequestedPreview } from "../../src/repositories/requested-previews";
import {
  invokeCalls,
  mockCommands,
  resetTauriMocks,
} from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks();
});

it("preserves the backend's coalesced result for concurrent callers", async () => {
  let calls = 0;
  mockCommands({
    ensure_preview: () => ({
      canonicalHash: "canonical-hash",
      coalesced: calls++ > 0,
    }),
  });

  const first = ensureRequestedPreview("same-hash");
  const second = ensureRequestedPreview("same-hash");

  await expect(Promise.all([first, second])).resolves.toEqual([
    "canonical-hash",
    "canonical-hash",
  ]);
  expect(
    invokeCalls.filter((call) => call.command === "ensure_preview"),
  ).toHaveLength(2);
  await vi.waitFor(() => expect(
    invokeCalls.filter((call) => call.command === "activity_record"),
  ).toHaveLength(4));
  const records = invokeCalls.filter((call) => call.command === "activity_record")
    .map((call) => call.args.draft as ActivityDraft);
  const starts = records.filter((record) => record.kind === "started");
  expect(starts).toHaveLength(2);
  expect(starts[0].operationId).not.toBe(starts[1].operationId);
  expect(records.filter((record) => record.kind !== "started")).toEqual([
    { owner: "preview", operationId: starts[0].operationId,
      kind: "completed", previous: "running", current: "succeeded", reason: "completion" },
    { owner: "preview", operationId: starts[1].operationId,
      kind: "coalesced", previous: "running", current: "coalesced", reason: "superseded" },
  ]);
});

it("does not coalesce different previews", async () => {
  mockCommands({
    ensure_preview: ({ hash }) => ({ canonicalHash: String(hash), coalesced: false }),
  });
  await Promise.all([
    ensureRequestedPreview("first-hash"),
    ensureRequestedPreview("second-hash"),
  ]);

  expect(
    invokeCalls.filter((call) => call.command === "ensure_preview"),
  ).toHaveLength(2);
  await vi.waitFor(() => expect(
    invokeCalls.filter((call) => call.command === "activity_record"),
  ).toHaveLength(4));
  const records = invokeCalls.filter((call) => call.command === "activity_record")
    .map((call) => call.args.draft as ActivityDraft);
  const starts = records.filter((record) => record.kind === "started");
  expect(new Set(starts.map((record) => record.operationId)).size).toBe(2);
  expect(records.filter((record) => record.kind !== "started")).toEqual(
    starts.map((start) => ({ owner: "preview", operationId: start.operationId,
      kind: "completed", previous: "running", current: "succeeded", reason: "completion" })),
  );
});
