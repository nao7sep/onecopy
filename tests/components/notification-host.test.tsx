// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import NotificationHost from "../../src/components/NotificationHost";
import {
  type NotificationRecord,
  useNotificationsStore,
} from "../../src/state/notifications-store";
import { useReleaseCheckStore } from "../../src/state/release-check-store";
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
  useReleaseCheckStore.setState({ noticeVersion: null, noticeLinkError: null });
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

  it("does not resume while the mouse leaves but focus is still inside, or focus leaves but the mouse is still over it (Finding C)", async () => {
    vi.useFakeTimers();
    render(<NotificationHost />);
    act(() =>
      useNotificationsStore.setState({
        active: [notice({ presentation: "timed", level: "info", message: "Done." })],
      }),
    );
    const noticeSurface = document.querySelector("[data-notification]") as HTMLElement;
    const dismiss = document.querySelector('[aria-label="Dismiss notification"]') as HTMLElement;

    // Mouse enters, then keyboard focus lands inside while the mouse is still
    // over it, then the mouse leaves — the still-held focus must keep it paused.
    fireEvent.mouseEnter(noticeSurface);
    fireEvent.focus(dismiss);
    fireEvent.mouseLeave(noticeSurface);
    await act(async () => vi.advanceTimersByTimeAsync(10_000));
    expect(document.body.textContent).toContain("Done.");

    // Now focus leaves too, with nothing left holding it: it resumes and
    // eventually dismisses.
    fireEvent.blur(dismiss);
    await act(async () => vi.advanceTimersByTimeAsync(6_000));
    expect(invokeCalls.some((call) => call.command === "dismiss_notification")).toBe(true);
  });

  it("never starts a timer for a window that does not own timed dismissal (Finding C)", async () => {
    vi.useFakeTimers();
    render(<NotificationHost ownsTimedDismissal={false} />);
    act(() =>
      useNotificationsStore.setState({
        active: [notice({ presentation: "timed", level: "info", message: "Done." })],
      }),
    );
    await act(async () => vi.advanceTimersByTimeAsync(60_000));
    expect(document.body.textContent).toContain("Done.");
    expect(invokeCalls.some((call) => call.command === "dismiss_notification")).toBe(false);
  });

  it("shows the coalesced occurrence count", () => {
    render(<NotificationHost />);
    act(() => useNotificationsStore.setState({ active: [notice({ occurrenceCount: 4 })] }));
    expect(document.body.textContent).toContain("Occurred 4 times");
  });

  it("keeps every active notice in a viewport-bounded local scroll region", () => {
    render(<NotificationHost />);
    act(() => useNotificationsStore.setState({
      active: Array.from({ length: 12 }, (_, index) => notice({
        id: index + 1,
        kind: `failure-${index + 1}`,
        message: `Failure ${index + 1}`,
      })),
    }));

    const host = document.querySelector("[data-notification-host]") as HTMLElement;
    const region = document.querySelector(
      "[data-notification-scroll-region]",
    ) as HTMLElement;
    expect(host.className).toContain("max-h-[calc(100vh-2rem)]");
    expect(region.className).toContain("overflow-y-auto");
    expect(region.className).toContain("overscroll-contain");
    expect(region.querySelectorAll("[data-notification]")).toHaveLength(12);
  });

  it("draws notice shadows from the host so the scroll region cannot clip them", () => {
    useReleaseCheckStore.setState({ noticeVersion: "9.9.9" });
    render(<NotificationHost />);
    act(() => useNotificationsStore.setState({ active: [notice()] }));

    const host = document.querySelector("[data-notification-host]") as HTMLElement;
    expect(host.className).toContain("[filter:drop-shadow(");
    for (const card of document.querySelectorAll("[data-notification], [data-release-notice]")) {
      expect(card.className).not.toMatch(/\bshadow-/);
    }
  });

  it("keeps the release notice outside the active-notification scroll owner", () => {
    useReleaseCheckStore.setState({ noticeVersion: "9.9.9" });
    render(<NotificationHost />);
    act(() => useNotificationsStore.setState({ active: [notice()] }));

    const release = document.querySelector("[data-release-notice]") as HTMLElement;
    const region = document.querySelector(
      "[data-notification-scroll-region]",
    ) as HTMLElement;
    expect(release.textContent).toContain("OneCopy 9.9.9 is available.");
    expect(region.contains(release)).toBe(false);
    expect(region.textContent).toContain("Couldn’t open the selected file.");
  });

  it("clears live notices when the reconstructible library index is rebuilt", async () => {
    render(<NotificationHost />);
    act(() => useNotificationsStore.setState({ active: [notice()] }));
    expect(document.body.textContent).toContain("Couldn’t open the selected file.");

    await act(async () => fireBackendEvent("notification://cleared"));
    expect(document.body.textContent).not.toContain("Couldn’t open the selected file.");
  });
});
