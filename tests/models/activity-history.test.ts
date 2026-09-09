import { expect, it } from "vitest";
import { mergeActivity, operationPresentation } from "../../src/models/activity-history";
import type { ActivityOperation } from "../../src/repositories/activity";

function row(id: number, revision = id): ActivityOperation {
  const event = { eventId: id, sessionId: "one", sequence: id, eventTimeUtc: "2026-09-09T00:00:00.000Z",
    monotonicMs: 10, kind: "started" as const, owner: "backgroundWork" as const, current: "running" as const };
  return { id, first: event, latest: { ...event, eventId: revision, monotonicMs: 100 }, started: event,
    progress: null, eventCount: 1, targetHash: null, target: null };
}

it("does not reorder completed operations or inject disconnected older pages", () => {
  expect(mergeActivity([row(10), row(9)], [row(9, 20), row(1, 21), row(11, 22)], true).map((row) => row.id)).toEqual([11, 10, 9]);
  expect(mergeActivity([row(9, 20)], [row(9, 9)], false)[0].latest.eventId).toBe(20);
});

it("never fabricates a start, terminal outcome, or running work from an earlier session", () => {
  expect(operationPresentation(row(1), "two", 9999).state).toContain("Interrupted");
  expect(operationPresentation({ ...row(1), started: null }, "one", 9999).duration).toBeNull();
  const closed = { ...row(1), latest: { ...row(1).latest, kind: "closed" as const, current: undefined } };
  expect(operationPresentation(closed, "one", 9999)).toMatchObject({ state: "Ended — outcome not recorded", duration: "90 ms" });
});

it("retains the last progress counts after a terminal event without counts", () => {
  const work = row(1);
  work.progress = { ...work.latest, done: 5, total: 5 };
  work.latest = { ...work.latest, kind: "completed", current: "succeeded" };
  expect(operationPresentation(work, "one", 100)).toMatchObject({ state: "Completed", progress: "5 / 5" });
});
