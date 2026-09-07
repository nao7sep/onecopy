import { beforeEach, expect, it } from "vitest";
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
  expect(
    invokeCalls.filter((call) => call.command === "activity_record"),
  ).toHaveLength(4);
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
});
