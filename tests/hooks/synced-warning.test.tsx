// @vitest-environment happy-dom
//
// A permanent deletion reaching a synced folder says so; the core answers
// from the items' live copies.

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";
import { useSyncedWarning } from "../../src/hooks/useSyncedWarning";
import { invokeCalls, mockCommand, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => resetTauriMocks());
afterEach(() => cleanup());

describe("the synced-folder warning", () => {
  it("asks the core about the reviewed items and shows its answer", async () => {
    mockCommand("items_in_synced_folders", () => true);
    const { result } = renderHook(() => useSyncedWarning(["h1", "path-7"]));
    await act(async () => {});
    expect(result.current).toBe(true);
    const asked = invokeCalls.find((call) => call.command === "items_in_synced_folders");
    expect(asked?.args.items).toEqual([{ hash: "h1" }, { pathId: 7 }]);
  });

  it("asks nothing when no deletion is under review", async () => {
    const { result } = renderHook(() => useSyncedWarning(null));
    await act(async () => {});
    expect(result.current).toBe(false);
    expect(invokeCalls.some((call) => call.command === "items_in_synced_folders")).toBe(false);
  });
});
