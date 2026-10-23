// @vitest-environment happy-dom
import { expect, it, vi } from "vitest";
import { installActivityPings } from "../../src/state/derived-work-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

it("pings immediately and keeps its elapsed interval across a wall-clock correction", () => {
  resetTauriMocks();
  mockCommands({ note_user_activity: () => null });
  let elapsed = 0;
  const now = vi.spyOn(performance, "now").mockImplementation(() => elapsed);
  const wall = vi.spyOn(Date, "now").mockReturnValue(3_600_000);
  const uninstall = installActivityPings(window);
  const count = () => invokeCalls.filter((call) => call.command === "note_user_activity").length;
  try {
    window.dispatchEvent(new Event("pointerdown"));
    expect(count()).toBe(1);
    wall.mockReturnValue(0);
    elapsed = 9_999;
    window.dispatchEvent(new Event("wheel"));
    expect(count()).toBe(1);
    elapsed = 10_000;
    window.dispatchEvent(new Event("keydown"));
    expect(count()).toBe(2);
  } finally {
    uninstall();
    now.mockRestore();
    wall.mockRestore();
  }
});
