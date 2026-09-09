// @vitest-environment happy-dom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import ActivityTraceModal from "../../src/components/ActivityTraceModal";
import { formatActivityTime } from "../../src/models/activity-history";
import type { ActivityEvent, ActivityOperation, ActivityPage } from "../../src/repositories/activity";
import { mockCommands, resetTauriMocks, invokeCalls } from "../mocks/tauri";

const event: ActivityEvent = {
  eventId: 12, sessionId: "session-one", sequence: 7,
  eventTimeUtc: "2026-09-07T00:00:00.000Z", monotonicMs: 42,
  kind: "started", owner: "backgroundWork", subject: "previews",
  operationId: "work:12", causeId: "priority:3", current: "running",
  done: 2, total: 10,
};
const operation = (id = 12): ActivityOperation => ({
  id, first: { ...event, eventId: id, operationId: "work:" + id },
  latest: { ...event, eventId: id, operationId: "work:" + id },
  started: { ...event, eventId: id }, progress: event, eventCount: 1, targetHash: null, target: null,
});
const page = (operations = [operation()], extra: Partial<ActivityPage> = {}): ActivityPage => ({
  operations, revision: 12, sessionId: "session-one", monotonicNowMs: 100, nextCursor: null, hasMore: false, ...extra,
});
beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    activity_page: ({ after }) => page(after === null ? [operation()] : []),
    activity_events: () => ({ events: [event], nextCursor: null }),
  });
});
afterEach(() => { cleanup(); vi.useRealTimers(); vi.restoreAllMocks(); });

it("shows ordinary work with local time, no JSONL export, and shared technical identifiers once", async () => {
  render(<ActivityTraceModal open onClose={() => {}} />);
  const row = await screen.findByRole("button", { name: /Prepare thumbnails and previews/ });
  expect(screen.getByText(formatActivityTime(event.eventTimeUtc))).toBeTruthy();
  expect(screen.queryByText("Copy JSONL")).toBeNull();
  expect(screen.queryByText("work:12")).toBeNull();
  fireEvent.click(row);
  expect(await screen.findByText("work:12")).toBeTruthy();
  expect(screen.getAllByText("session-one")).toHaveLength(1);
  expect(screen.getAllByText("priority:3")).toHaveLength(1);
});

it("drains a burst through the forward cursor and updates an old operation in place", async () => {
  vi.useFakeTimers();
  mockCommands({
    activity_page: ({ after }) => after === null ? page()
      : after === 12 ? page([operation(13)], { revision: 13, hasMore: true })
      : page([{ ...operation(), latest: { ...event, eventId: 14, kind: "completed", current: "succeeded" }, eventCount: 2 }], { revision: 14 }),
  });
  render(<ActivityTraceModal open onClose={() => {}} />);
  await act(async () => {});
  await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
  expect(invokeCalls.filter((call) => call.command === "activity_page").map((call) => call.args?.after)).toEqual([null, 12, 13]);
  expect(screen.getByText("2 operations loaded")).toBeTruthy();
  const rows = screen.getAllByRole("button", { name: /Prepare thumbnails/ });
  expect(rows[0].dataset.activityAnchor).toBe("operation:13");
  expect(rows[1].textContent).toContain("Completed");
});

it("ignores older-page completion after close and reopen", async () => {
  let finish!: (value: ActivityPage) => void;
  mockCommands({ activity_page: ({ before }) => before === null ? page([operation()], { nextCursor: 12 })
    : new Promise<ActivityPage>((resolve) => { finish = resolve; }) });
  const view = render(<ActivityTraceModal open onClose={() => {}} />);
  await screen.findByText("1 operations loaded · scroll for older activity");
  fireEvent.scroll(screen.getByRole("region", { name: "Activity history" }));
  await waitFor(() => expect(finish).toBeDefined());
  view.rerender(<ActivityTraceModal open={false} onClose={() => {}} />);
  mockCommands({ activity_page: () => page([operation(99)], { revision: 99 }) });
  view.rerender(<ActivityTraceModal open onClose={() => {}} />);
  await screen.findByText("1 operations loaded");
  await act(async () => { finish(page([operation(1)])); });
  expect(screen.getAllByRole("button", { name: /Prepare thumbnails/ })).toHaveLength(1);
  expect(screen.getByRole("button", { name: /Prepare thumbnails/ }).dataset.activityAnchor).toBe("operation:99");
});

it("keeps the visible operation at its pixel offset when a newer row arrives", async () => {
  vi.useFakeTimers();
  let updates = false;
  mockCommands({ activity_page: ({ after }) => after === null ? page([operation(12), operation(11)])
    : page(updates ? [operation(13)] : [], { revision: updates ? 13 : 12 }) });
  render(<ActivityTraceModal open onClose={() => {}} />);
  await act(async () => {});
  const viewport = screen.getByRole("region", { name: "Activity history" });
  viewport.scrollTop = 40;
  const rect = (top: number, height = 40) => ({ top, bottom: top + height, height, left: 0, right: 200, width: 200, x: 0, y: top, toJSON() {} });
  vi.spyOn(viewport, "getBoundingClientRect").mockImplementation(() => rect(0, 100));
  for (const row of screen.getAllByRole("button", { name: /Prepare thumbnails/ })) {
    vi.spyOn(row, "getBoundingClientRect").mockImplementation(() =>
      rect((row.dataset.activityAnchor === "operation:12" ? 0 : 40) + (document.querySelector('[data-activity-anchor="operation:13"]') ? 40 : 0) - viewport.scrollTop));
  }
  updates = true;
  await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
  expect(viewport.scrollTop).toBe(80);
});

it("keeps an older-page failure visible across successful live refreshes", async () => {
  vi.useFakeTimers();
  mockCommands({ activity_page: ({ before, after }) => {
    if (before !== null) throw new Error("older read failed");
    return page(after === null ? [operation()] : [], { nextCursor: 12 });
  } });
  render(<ActivityTraceModal open onClose={() => {}} />);
  await act(async () => {});
  fireEvent.scroll(screen.getByRole("region", { name: "Activity history" }));
  await act(async () => {});
  expect(screen.getByText(/Older activity could not be loaded/)).toBeTruthy();
  await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
  expect(screen.getByText(/Older activity could not be loaded/)).toBeTruthy();
});

it("finishes a slow technical read and catches intervening events without restarting it", async () => {
  vi.useFakeTimers();
  let finish!: (value: unknown) => void;
  let revision = 12;
  mockCommands({
    activity_page: () => page([{ ...operation(), latest: { ...event, eventId: revision } }], { revision }),
    activity_events: () => new Promise((resolve) => { finish = resolve; }),
  });
  render(<ActivityTraceModal open onClose={() => {}} />);
  await act(async () => {});
  fireEvent.click(screen.getByRole("button", { name: /Prepare thumbnails/ }));
  await act(async () => {});
  revision = 14;
  await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
  expect(invokeCalls.filter((call) => call.command === "activity_events")).toHaveLength(1);
  await act(async () => { finish({ events: [event], nextCursor: null }); });
  mockCommands({ activity_events: ({ before }) => before === null
    ? { events: [{ ...event, eventId: 14, kind: "completed", current: "succeeded" }], nextCursor: 14 }
    : { events: [{ ...event, eventId: 13, kind: "progressed" }, event], nextCursor: null } });
  await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
  expect(invokeCalls.filter((call) => call.command === "activity_events").map((call) => call.args?.before)).toEqual([null, null, 14]);
  expect(document.querySelectorAll('[data-activity-anchor^="event:"]')).toHaveLength(3);
});
