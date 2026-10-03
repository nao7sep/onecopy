import { describe, expect, it } from "vitest";
import {
  cursorAfter,
  fieldPresentation,
  mergeNewestPage,
  newestFirst,
  prettyJson,
  recordSentence,
  type RecordSummary,
} from "../../src/models/records";

function summary(kind: RecordSummary["kind"], id: number, time: string): RecordSummary {
  return { kind, id, session: null, time, level: "info", title: `${kind} ${id}`, text: null, messageKey: null, messageValues: null };
}

const at = (second: number) => `2026-10-02T08:00:${String(second).padStart(2, "0")}.000Z`;

describe("records", () => {
  it("orders by time, then kind name, then id, newest first, as the core pages", () => {
    const rows = [summary("issue", 3, at(1)), summary("log", 2, at(1)), summary("log", 5, at(1)), summary("notice", 1, at(0))];
    expect([...rows].sort(newestFirst).map((row) => row.title)).toEqual(["log 5", "log 2", "issue 3", "notice 1"]);
  });

  it("starts the next page after the last row shown", () => {
    expect(cursorAfter([])).toBeNull();
    expect(cursorAfter([summary("log", 2, at(2)), summary("trash", 7, at(1))])).toEqual({ time: at(1), kind: "trash", id: 7 });
  });

  it("joins a re-read newest page with the rows already shown", () => {
    const shown = [summary("log", 3, at(3)), summary("log", 2, at(2)), summary("log", 1, at(1))];
    const updated = { ...summary("log", 3, at(3)), title: "updated" };
    const page = { records: [summary("log", 4, at(4)), updated], more: true };

    const merged = mergeNewestPage(shown, false, page);

    expect(merged.records.map((row) => row.title)).toEqual(["log 4", "updated", "log 2", "log 1"]);
    // Rows beyond the page are still the ones shown, so their own `more` holds.
    expect(merged.more).toBe(false);
    expect(mergeNewestPage([], false, page).more).toBe(true);
  });

  it("indents stored JSON and leaves other text as it is", () => {
    expect(prettyJson('{"a":1}')).toBe('{\n  "a": 1\n}');
    expect(prettyJson("not json")).toBe("not json");
  });

  it("names each stored column, and an activity's kind as its event", () => {
    expect(fieldPresentation("log", "line")).toEqual({ label: "records.line", shape: "json" });
    expect(fieldPresentation("activity", "kind")).toEqual({ label: "records.event", shape: "value" });
    expect(fieldPresentation("notice", "kind")).toEqual({ label: "records.condition", shape: "value" });
    expect(fieldPresentation("activity", "monotonic_ms")).toEqual({ label: "records.sinceLaunch", shape: "milliseconds" });
    expect(fieldPresentation("notice", "message_key")).toEqual({ label: "records.shownText", shape: "sentence" });
    expect(fieldPresentation("log", "toString")).toBeNull();
    expect(fieldPresentation("log", "a_column_from_a_newer_build")).toBeNull();
  });

  it("reads a recorded sentence from its key and stored values", () => {
    expect(recordSentence({ messageKey: null, messageValues: null })).toBeNull();
    expect(recordSentence({ messageKey: "notice.previewFailed", messageValues: '{"name":"a.jpg","nested":{"x":1}}' }))
      .toEqual({ key: "notice.previewFailed", values: { name: "a.jpg" } });
    expect(recordSentence({ messageKey: "notice.previewFailed", messageValues: "not json" }))
      .toEqual({ key: "notice.previewFailed" });
  });
});
