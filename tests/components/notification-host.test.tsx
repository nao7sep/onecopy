// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import NotificationHost from "../../src/components/NotificationHost";
import {
  type NotificationRecord,
  useNotificationsStore,
} from "../../src/state/notifications-store";
import {
  fireEvent as fireBackendEvent,
  invokeCalls,
  mockCommands,
  resetTauriMocks,
} from "../mocks/tauri";

function notice(over: Partial<NotificationRecord> = {}): NotificationRecord {
  return {
    id: 7,
    kind: "open-failed",
    path: null,
    level: "error",
    presentation: "persistent",
    message: "Couldn’t open the selected file.",
    firstSeenUtc: "2026-08-31T00:00:00.000Z",
    lastSeenUtc: "2026-08-31T00:00:00.000Z",
    occurrenceCount: 1,
    ...over,
  };
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    get_active_notifications: () => [],
    dismiss_notification: () => true,
  });
  useNotificationsStore.setState({ active: [], dismissing: new Set() });
});

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

describe("the app-frame notification host", () => {
  it("keeps a persistent notice visible until its explicit dismiss action", async () => {
    render(<NotificationHost />);
    act(() => useNotificationsStore.setState({ active: [notice()] }));

    expect(document.body.textContent).toContain("Couldn’t open the selected file.");
    await act(async () => {
      (document.querySelector('[aria-label="Dismiss notification"]') as HTMLElement).click();
    });

    expect(invokeCalls.some((call) => call.command === "dismiss_notification")).toBe(true);
    expect(document.body.textContent).not.toContain("Couldn’t open the selected file.");
  });

  it("dismisses a timed notice after the configured default interval", async () => {
    vi.useFakeTimers();
    render(<NotificationHost />);
    act(() =>
      useNotificationsStore.setState({
        active: [notice({ presentation: "timed", level: "info", message: "Done." })],
      }),
    );

    await act(async () => vi.advanceTimersByTimeAsync(5_999));
    expect(document.body.textContent).toContain("Done.");
    await act(async () => vi.advanceTimersByTimeAsync(1));
    expect(invokeCalls.some((call) => call.command === "dismiss_notification")).toBe(true);
  });

  it("pauses a timed notice while it is hovered or focused", async () => {
    vi.useFakeTimers();
    render(<NotificationHost />);
    act(() =>
      useNotificationsStore.setState({
        active: [notice({ presentation: "timed", level: "info", message: "Done." })],
      }),
    );
    const noticeSurface = document.querySelector("[data-notification]") as HTMLElement;
    const dismiss = document.querySelector('[aria-label="Dismiss notification"]') as HTMLElement;

    fireEvent.mouseEnter(noticeSurface);
    await act(async () => vi.advanceTimersByTimeAsync(7_000));
    expect(document.body.textContent).toContain("Done.");

    fireEvent.mouseLeave(noticeSurface);
    await act(async () => vi.advanceTimersByTimeAsync(1_000));
    fireEvent.focus(dismiss);
    await act(async () => vi.advanceTimersByTimeAsync(7_000));
    expect(document.body.textContent).toContain("Done.");

    fireEvent.blur(dismiss);
    await act(async () => vi.advanceTimersByTimeAsync(4_999));
    expect(document.body.textContent).toContain("Done.");
    await act(async () => vi.advanceTimersByTimeAsync(1));
    expect(invokeCalls.some((call) => call.command === "dismiss_notification")).toBe(true);
  });

  it("shows the coalesced occurrence count", () => {
    render(<NotificationHost />);
    act(() => useNotificationsStore.setState({ active: [notice({ occurrenceCount: 4 })] }));
    expect(document.body.textContent).toContain("Occurred 4 times");
  });

  it("clears live notices when the reconstructible library index is rebuilt", async () => {
    render(<NotificationHost />);
    act(() => useNotificationsStore.setState({ active: [notice()] }));
    expect(document.body.textContent).toContain("Couldn’t open the selected file.");

    await act(async () => fireBackendEvent("notification://cleared"));
    expect(document.body.textContent).not.toContain("Couldn’t open the selected file.");
  });
});
