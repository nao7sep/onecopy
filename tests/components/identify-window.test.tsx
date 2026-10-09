// @vitest-environment happy-dom

import { cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import IdentifyWindow from "../../src/windows/IdentifyWindow";
import { close, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks();
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

// The identify flash is self-closing — a flash, not a
// surface a user dismisses — and must not linger or close early.
describe("identify flash window", () => {
  it("closes itself after its timeout, not before", async () => {
    vi.useFakeTimers();
    render(<IdentifyWindow number={2} />);

    await vi.advanceTimersByTimeAsync(2199);
    expect(close).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1);
    expect(close).toHaveBeenCalledTimes(1);
  });

  it("cancels its timer on unmount so it never closes a reused window late", async () => {
    vi.useFakeTimers();
    const view = render(<IdentifyWindow number={1} />);
    view.unmount();

    await vi.advanceTimersByTimeAsync(3000);
    expect(close).not.toHaveBeenCalled();
  });
});
