import { describe, expect, it, vi } from "vitest";
import type { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { waitForWindowCreated } from "../../src/utils/windowCreation";

function fakeWindow() {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();
  const stops = new Map<string, ReturnType<typeof vi.fn>>();
  const once = vi.fn(
    async (event: string, handler: (event: { payload: unknown }) => void) => {
      handlers.set(event, handler);
      const stop = vi.fn(() => handlers.delete(event));
      stops.set(event, stop);
      return stop;
    },
  );
  return {
    window: { once } as unknown as WebviewWindow,
    handlers,
    stops,
  };
}

describe("window creation wait", () => {
  it("settles on creation and releases both outcome listeners", async () => {
    const fixture = fakeWindow();
    const waiting = waitForWindowCreated(fixture.window, "Preview");
    await Promise.resolve();
    fixture.handlers.get("tauri://created")?.({ payload: null });

    await expect(waiting).resolves.toBeUndefined();
    expect(fixture.stops.get("tauri://created")).toHaveBeenCalledOnce();
    expect(fixture.stops.get("tauri://error")).toHaveBeenCalledOnce();
  });

  it("rejects a missing native outcome after a bounded wait", async () => {
    vi.useFakeTimers();
    try {
      const fixture = fakeWindow();
      const waiting = waitForWindowCreated(fixture.window, "Viewer");
      const rejected = expect(waiting).rejects.toThrow(
        "Viewer window creation timed out",
      );

      await vi.advanceTimersByTimeAsync(10_000);
      await rejected;
      expect(fixture.stops.get("tauri://created")).toHaveBeenCalledOnce();
      expect(fixture.stops.get("tauri://error")).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });
});
