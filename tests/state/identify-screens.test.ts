// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Monitor } from "@tauri-apps/api/window";
import { identifyScreens } from "../../src/workflows/identify-screens";
import { createdWindows, resetTauriMocks, WebviewWindow } from "../mocks/tauri";

const monitors = [0, 1, 2].map((index) => ({
  name: `Display ${index}`, position: { x: index * 1920, y: 0 },
  size: { width: 1920, height: 1080 }, scaleFactor: 2,
})) as Monitor[];

async function outcome(label: string, name: string, payload: unknown = null) {
  const window = (await WebviewWindow.getByLabel(label))!;
  const handlers = window.once.mock.calls as unknown as Array<[string, (event: { payload: unknown }) => void]>;
  handlers.find(([event]) => event === name)![1]({ payload });
  return window;
}

beforeEach(() => { resetTauriMocks(); vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); });

describe("screen identification operation", () => {
  it("settles creation once, ignores late errors, and never reuses a closing flash identity", async () => {
    const first = identifyScreens(monitors);
    expect(identifyScreens(monitors)).toBe(first);
    expect(createdWindows).toHaveLength(3);
    const firstLabels = createdWindows.map(({ label }) => label);
    for (const [index, label] of firstLabels.entries()) {
      const window = await outcome(label, "tauri://created");
      expect(createdWindows[index].options).toMatchObject({
        url: `index.html?view=identify&slice=${index + 1}`,
        x: index * 960 + 370, y: 160, focus: false, width: 220, height: 220,
      });
      expect(window.setFocus).not.toHaveBeenCalled();
    }
    await expect(first).resolves.toBeUndefined();
    // Even an already-dispatched stale callback cannot change success.
    await outcome(firstLabels[0], "tauri://error", "late teardown error");
    const second = identifyScreens(monitors);
    const secondLabels = createdWindows.slice(3).map(({ label }) => label);
    expect(secondLabels.every((label) => !firstLabels.includes(label))).toBe(true);
    for (const label of secondLabels) await outcome(label, "tauri://created");
    await expect(second).resolves.toBeUndefined();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("contains partial creation failures, waits for every outcome, and permits a fresh retry", async () => {
    const failed = identifyScreens(monitors);
    const rejection = expect(failed).rejects.toMatchObject({
      message: "Screen identification failed",
      cause: [{ error: { message: "native fixture failure" } }],
    });
    await outcome(createdWindows[0].label, "tauri://error", "native fixture failure");
    expect(identifyScreens(monitors)).toBe(failed);
    for (const { label } of createdWindows.slice(1)) await outcome(label, "tauri://created");
    await rejection;
    const retry = identifyScreens(monitors.slice(0, 1));
    await outcome(createdWindows.at(-1)!.label, "tauri://created");
    await expect(retry).resolves.toBeUndefined();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("bounds missing creation outcomes and accepts a subsequent invocation", async () => {
    const failed = identifyScreens(monitors.slice(0, 1));
    const rejection = expect(failed).rejects.toThrow("Screen identification failed");
    await vi.advanceTimersByTimeAsync(10_000);
    await rejection;
    const retry = identifyScreens(monitors.slice(0, 1));
    await outcome(createdWindows.at(-1)!.label, "tauri://created");
    await retry;
    expect(vi.getTimerCount()).toBe(0);
  });
});
