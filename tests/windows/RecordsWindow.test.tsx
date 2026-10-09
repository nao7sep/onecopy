// @vitest-environment happy-dom
//
// The Records window over records.sqlite3: the list, its filters and paging,
// the selected record whole, live updates, and the list pane's saved width.

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import RecordsWindowRoute, { RecordsWindow } from "../../src/windows/RecordsWindow";
import { createTranslator } from "../../src/i18n/translate";
import type { RecordDetail, RecordsPage, RecordsQuery, RecordSummary } from "../../src/models/records";
import { RECORDS_CHANGED_EVENT, RECORDS_READ_TIMEOUT_MS } from "../../src/repositories/records";
import {
  RECORDS_DETAIL_MIN_WIDTH,
  RECORDS_LIST_WIDTH,
  SPLITTER_WIDTH,
  computeRecordsMinHeight,
  computeRecordsMinWidth,
} from "../../src/utils/windowSizing";
import {
  fireEvent as fireBackendEvent,
  invokeCalls,
  listenerCount,
  mockCommand,
  mockCommands,
  resetTauriMocks,
  setMinSize,
} from "../mocks/tauri";

const SESSION = "2026-10-02T08:00:00.000Z";
const PREVIEW_FAILED = createTranslator("en").t("notice.previewFailed", { name: "a.jpg" });
const OLDER_SESSION = "2026-10-01T08:00:00.000Z";

const issue: RecordSummary = {
  kind: "issue", id: 4, session: SESSION, time: "2026-10-02T08:01:00.000Z", level: "warn",
  title: "preview-failed occurred", text: "no decoder",
  messageKey: "notice.previewFailed", messageValues: JSON.stringify({ name: "a.jpg" }),
};
const line: RecordSummary = {
  kind: "log", id: 9, session: SESSION, time: "2026-10-02T08:00:30.000Z", level: "error",
  title: "boundary failed", text: "disk full", messageKey: null, messageValues: null,
};
const newer: RecordSummary = {
  kind: "log", id: 12, session: SESSION, time: "2026-10-02T08:02:00.000Z", level: "info",
  title: "app later", text: null, messageKey: null, messageValues: null,
};
const issueDetail: RecordDetail = {
  ...issue,
  fields: [
    { name: "session_id", value: SESSION },
    { name: "time_utc", value: issue.time },
    { name: "kind", value: "preview-failed" },
    { name: "path", value: "/photos/a.jpg" },
    { name: "event", value: "occurred" },
    { name: "message", value: "no decoder" },
    { name: "message_key", value: "notice.previewFailed" },
    { name: "message_values", value: JSON.stringify({ name: "a.jpg" }) },
  ],
};

let pages: Array<RecordsPage | Promise<RecordsPage> | Error>;
let defaultPage: RecordsPage;

// happy-dom lays nothing out, so the list's scroll box and the window's width
// are set here. By default the list is scrolled to the top and far from its end.
const box = { scrollTop: 0, scrollHeight: 1000, clientHeight: 200, shellWidth: 2000 };
const resizeCallbacks = new Set<() => void>();
class TestResizeObserver {
  private readonly callback: () => void;
  constructor(callback: () => void) {
    this.callback = callback;
  }
  observe(): void {
    resizeCallbacks.add(this.callback);
  }
  disconnect(): void {
    resizeCallbacks.delete(this.callback);
  }
}
const isScroll = (element: HTMLElement) => element.hasAttribute("data-records-scroll");

beforeEach(() => {
  resetTauriMocks();
  pages = [];
  defaultPage = { records: [issue, line], more: false };
  mockCommands({
    log_event: () => null,
    records_page: () => {
      const next = pages.shift() ?? defaultPage;
      if (next instanceof Error) throw next;
      return next;
    },
    records_detail: () => issueDetail,
    records_sources: () => ({ currentSession: SESSION, sessions: [SESSION, OLDER_SESSION] }),
    patch_state: ({ patch }) => patch,
    records_list_width: () => null,
  });
  Object.assign(box, { scrollTop: 0, scrollHeight: 1000, clientHeight: 200, shellWidth: 2000 });
  resizeCallbacks.clear();
  vi.stubGlobal("ResizeObserver", TestResizeObserver);
  Object.defineProperties(HTMLElement.prototype, {
    scrollTop: {
      configurable: true,
      get(this: HTMLElement) { return isScroll(this) ? box.scrollTop : 0; },
      set(this: HTMLElement, value: number) { if (isScroll(this)) box.scrollTop = value; },
    },
    scrollHeight: { configurable: true, get(this: HTMLElement) { return isScroll(this) ? box.scrollHeight : 0; } },
    clientHeight: { configurable: true, get(this: HTMLElement) { return isScroll(this) ? box.clientHeight : 0; } },
    clientWidth: {
      configurable: true,
      get(this: HTMLElement) { return this.parentElement?.id === "root-under-test" ? box.shellWidth : 0; },
    },
  });
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  for (const name of ["scrollTop", "scrollHeight", "clientHeight", "clientWidth"]) {
    delete (HTMLElement.prototype as unknown as Record<string, unknown>)[name];
  }
});

const settle = async () => {
  for (let index = 0; index < 5; index++) await act(async () => {});
};

async function mount(): Promise<void> {
  const container = document.createElement("div");
  container.id = "root-under-test";
  document.body.append(container);
  render(<RecordsWindow initialListWidth={RECORDS_LIST_WIDTH.default} />, { container });
  await settle();
}

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

const options = () => Array.from(document.querySelectorAll<HTMLElement>('[role="option"]'));
const titles = () => options().map((option) => option.querySelector("[data-record-title]")?.textContent);
const pageCalls = () => invokeCalls.filter((call) => call.command === "records_page");
const lastQuery = (): RecordsQuery => pageCalls().at(-1)!.args.query as RecordsQuery;
const listbox = () => document.querySelector<HTMLElement>('[role="listbox"]')!;
const scrollBox = () => document.querySelector<HTMLElement>("[data-records-scroll]")!;
const shownListWidth = () => document.querySelector<HTMLElement>("[data-records-list-pane]")!.style.width;
const scrollTo = async (top: number, events = 1) => {
  await act(async () => {
    box.scrollTop = top;
    for (let index = 0; index < events; index++) scrollBox().dispatchEvent(new Event("scroll"));
  });
  await settle();
};
const press = async (key: string) => {
  fireEvent.keyDown(listbox(), { key });
  await settle();
};
const signal = async () => {
  await act(async () => fireBackendEvent(RECORDS_CHANGED_EVENT));
};
const tick = async (ms = 1000) => {
  await act(async () => {
    vi.advanceTimersByTime(ms);
  });
  for (let index = 0; index < 5; index++) await act(async () => {});
};
const cursorOf = (record: RecordSummary) => ({ time: record.time, kind: record.kind, id: record.id });
const NO_FILTERS = { session: null, kind: null, level: null, search: "", after: null };

describe("the Records window", () => {
  it("lists the records newest first, with nothing selected yet", async () => {
    await mount();

    expect(titles()).toEqual(["preview-failed occurred", "boundary failed"]);
    expect(options()[0]!.textContent).toContain(PREVIEW_FAILED);
    expect(options()[1]!.textContent).toContain("disk full");
    expect(lastQuery()).toEqual(NO_FILTERS);
    expect(document.body.textContent).toContain("Select a record to see everything it holds.");
    expect(options().every((option) => option.getAttribute("aria-selected") === "false")).toBe(true);
    // One tab stop: the list, not its rows.
    expect(listbox().tabIndex).toBe(0);
    expect(options().every((option) => !option.hasAttribute("tabindex"))).toBe(true);
  });

  it("shows everything a selected record holds, as stored", async () => {
    await mount();
    fireEvent.mouseDown(options()[0]!);
    await settle();

    expect(invokeCalls.some((call) => call.command === "records_detail"
      && call.args.kind === "issue" && call.args.id === 4)).toBe(true);
    const body = document.querySelector("[data-records-detail-body]")!;
    const terms = Array.from(body.querySelectorAll("dt")).map((term) => term.textContent);
    expect(terms).toEqual(["Launch", "Time", "Condition", "Path", "Event", "Message", "Shown text"]);
    expect(body.textContent).toContain("(this launch)");
    expect(body.textContent).toContain("preview-failed");
    // The recorded sentence, in the interface language.
    expect(body.textContent).toContain(PREVIEW_FAILED);
    const blocks = Array.from(body.querySelectorAll("[data-records-block]")).map((block) => [
      block.querySelector("h3")?.textContent,
      block.querySelector("pre")?.textContent,
    ]);
    expect(blocks).toEqual([["Message values", JSON.stringify({ name: "a.jpg" }, null, 2)]]);
    expect(options()[0]!.getAttribute("aria-selected")).toBe("true");
    expect(listbox().getAttribute("aria-activedescendant")).toBe(options()[0]!.id);
  });

  it("moves the selection with the arrow keys, and Tab in selects the first record", async () => {
    await mount();
    await act(async () => listbox().focus());
    await settle();
    expect(options()[0]!.getAttribute("aria-selected")).toBe("true");

    await press("ArrowDown");

    expect(options()[1]!.getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(listbox());
    const detailCall = invokeCalls.filter((call) => call.command === "records_detail").at(-1)!;
    expect(detailCall.args).toEqual({ kind: "log", id: 9 });
  });

  it("reads again with each filter, and searches once typing pauses", async () => {
    await mount();
    const selects = Array.from(document.querySelectorAll("select"));
    expect(Array.from(selects[0]!.options).map((option) => option.textContent)).toEqual([
      "All launches",
      expect.stringContaining("(this launch)"),
      expect.not.stringContaining("(this launch)"),
    ]);
    expect(Array.from(selects[1]!.options).map((option) => option.textContent)).toEqual([
      "All kinds", "Log line", "Activity", "Deleted files", "Analysis", "Issue", "Notification",
    ]);

    fireEvent.change(selects[0]!, { target: { value: SESSION } });
    await settle();
    fireEvent.change(selects[1]!, { target: { value: "trash" } });
    await settle();
    fireEvent.change(selects[2]!, { target: { value: "error" } });
    await settle();
    expect(lastQuery()).toEqual({ ...NO_FILTERS, session: SESSION, kind: "trash", level: "error" });

    vi.useFakeTimers();
    const search = document.querySelector<HTMLInputElement>('input[type="search"]')!;
    fireEvent.change(search, { target: { value: "quota" } });
    expect(lastQuery().search).toBe("");
    await tick(300);
    expect(lastQuery().search).toBe("quota");
  });

  it("offers Needs attention first among the levels, with every filter off", async () => {
    await mount();
    const level = document.querySelectorAll("select")[2]!;
    expect(Array.from(level.options).map((option) => option.textContent)).toEqual([
      "All levels", "Needs attention", "Error", "Warning", "Info", "Debug",
    ]);
    expect(Array.from(document.querySelectorAll("select")).map((select) => select.value)).toEqual(["", "", ""]);
  });

  it("shows a loading note while the first page is read, then the rows", async () => {
    const first = deferred<RecordsPage>();
    pages.push(first.promise);
    await mount();

    expect(document.body.textContent).toContain("Loading records…");
    expect(document.body.textContent).not.toContain("No records match these filters.");
    expect(options()).toHaveLength(0);

    await act(async () => first.resolve({ records: [issue, line], more: false }));
    await settle();
    expect(options()).toHaveLength(2);
    expect(document.body.textContent).not.toContain("Loading records…");
  });

  it("says when no record matches, and when a record appears", async () => {
    defaultPage = { records: [], more: false };
    await mount();
    expect(document.body.textContent).toContain("No records match these filters.");

    vi.useFakeTimers();
    pages.push({ records: [newer], more: false });
    await signal();
    await tick();
    expect(titles()).toEqual(["app later"]);
    expect(document.body.textContent).not.toContain("No records match these filters.");
  });

  it("has no button of its own", async () => {
    await mount();
    expect(document.querySelectorAll("button")).toHaveLength(0);
  });

  it("reads the next page from the last row once the list is scrolled near its end", async () => {
    pages.push({ records: [issue], more: true }, { records: [line], more: false });
    await mount();
    expect(pageCalls()).toHaveLength(1);

    await scrollTo(700);

    expect(pageCalls()).toHaveLength(2);
    expect(lastQuery().after).toEqual(cursorOf(issue));
    expect(titles()).toEqual(["preview-failed occurred", "boundary failed"]);
  });

  it("reads the next page when ArrowDown is pressed on the last row", async () => {
    pages.push({ records: [issue, line], more: true }, { records: [], more: false });
    await mount();
    fireEvent.mouseDown(options()[1]!);
    await settle();
    await press("ArrowDown");

    expect(pageCalls()).toHaveLength(2);
    expect(lastQuery().after).toEqual(cursorOf(line));
    expect(options()[1]!.getAttribute("aria-selected")).toBe("true");
  });

  it("makes one request for two scroll events together", async () => {
    pages.push({ records: [issue, line], more: true }, new Promise<RecordsPage>(() => {}));
    await mount();

    await scrollTo(800, 2);

    expect(pageCalls()).toHaveLength(2);
    expect(options()).toHaveLength(2);
    expect(document.body.textContent).toContain("Loading records…");
  });

  it("reads the next page by itself while a page does not fill the list", async () => {
    box.scrollHeight = 150;
    pages.push({ records: [issue], more: true }, { records: [line], more: false });
    await mount();

    expect(pageCalls()).toHaveLength(2);
    expect(titles()).toEqual(["preview-failed occurred", "boundary failed"]);
  });

  it("keeps a failed page's note at the end, and reads it again when the end is reached again", async () => {
    pages.push({ records: [issue], more: true }, new Error("busy"), { records: [line], more: false });
    await mount();

    await scrollTo(700);
    expect(document.body.textContent).toContain("The records could not be read.");
    expect(options()).toHaveLength(1);
    expect(pageCalls()).toHaveLength(2);

    await scrollTo(750);
    expect(pageCalls()).toHaveLength(3);
    expect(lastQuery().after).toEqual(cursorOf(issue));
    expect(titles()).toEqual(["preview-failed occurred", "boundary failed"]);
    expect(document.body.textContent).not.toContain("The records could not be read.");
  });

  it("re-reads the newest page once for a burst of new records while at the top, keeping the rows shown", async () => {
    await mount();
    vi.useFakeTimers();
    const next = deferred<RecordsPage>();
    pages.push(next.promise);

    await signal();
    await signal();
    await signal();
    await tick();

    expect(pageCalls()).toHaveLength(2);
    expect(lastQuery()).toEqual(NO_FILTERS);
    expect(options()).toHaveLength(2);
    expect(document.body.textContent).not.toContain("Loading records…");

    await act(async () => next.resolve({ records: [newer, issue, line], more: false }));
    await settle();
    expect(titles()).toEqual(["app later", "preview-failed occurred", "boundary failed"]);
  });

  it("leaves the list alone while scrolled down, and shows new records once back at the top", async () => {
    await mount();
    await scrollTo(300);
    vi.useFakeTimers();
    pages.push({ records: [newer, issue, line], more: false });

    await signal();
    await tick();
    expect(pageCalls()).toHaveLength(1);
    expect(options()).toHaveLength(2);

    await scrollTo(0);
    expect(pageCalls()).toHaveLength(2);
    expect(titles()).toEqual(["app later", "preview-failed occurred", "boundary failed"]);
  });

  it("keeps the selected record selected through an update", async () => {
    await mount();
    fireEvent.mouseDown(options()[1]!);
    await settle();
    vi.useFakeTimers();
    pages.push({ records: [newer, issue, line], more: false });

    await signal();
    await tick();

    expect(options()).toHaveLength(3);
    expect(options()[2]!.getAttribute("aria-selected")).toBe("true");
    expect(invokeCalls.filter((call) => call.command === "records_detail")).toHaveLength(1);
  });

  it("stops reading on new-record signals after a failed read, so a logged failure cannot start the next read", async () => {
    await mount();
    vi.useFakeTimers();
    pages.push(new Error("busy"));

    await signal();
    await tick();
    expect(pageCalls()).toHaveLength(2);
    expect(invokeCalls.some((call) => call.command === "log_event")).toBe(true);

    await signal();
    await tick();
    expect(pageCalls()).toHaveLength(2);
    expect(options()).toHaveLength(2);
  });

  it("makes no record of its own reads", async () => {
    await mount();
    fireEvent.mouseDown(options()[0]!);
    await settle();
    await scrollTo(10);

    expect(invokeCalls.filter((call) => call.command === "log_event")).toEqual([]);
  });

  it("stops listening for new records when it closes", async () => {
    await mount();
    expect(listenerCount(RECORDS_CHANGED_EVENT)).toBe(1);
    cleanup();
    await settle();
    expect(listenerCount(RECORDS_CHANGED_EVENT)).toBe(0);
  });

  it("saves the list width once when a drag ends, clamped to the pane's bounds", async () => {
    await mount();
    const splitter = document.querySelector<HTMLElement>('[role="separator"]')!;
    expect(splitter.getAttribute("aria-label")).toBe("Resize list pane");

    fireEvent.mouseDown(splitter, { clientX: 0 });
    fireEvent.mouseMove(document, { clientX: 100 });
    fireEvent.mouseMove(document, { clientX: 2000 });
    fireEvent.mouseUp(document, { clientX: 2000 });
    await settle();

    const saves = invokeCalls.filter((call) => call.command === "patch_state");
    expect(saves.map((call) => call.args)).toEqual([{ patch: { recordsListWidth: RECORDS_LIST_WIDTH.max } }]);
    expect(shownListWidth()).toBe(`${RECORDS_LIST_WIDTH.max}px`);
  });

  it("resizes the list by keyboard and saves the width once when the key is released", async () => {
    await mount();
    const splitter = document.querySelector<HTMLElement>('[role="separator"]')!;
    expect(splitter.tabIndex).toBe(0);

    fireEvent.keyDown(splitter, { key: "ArrowRight" });
    fireEvent.keyDown(splitter, { key: "ArrowRight" });
    await settle();
    expect(shownListWidth()).toBe(`${RECORDS_LIST_WIDTH.default + 32}px`);
    expect(invokeCalls.some((call) => call.command === "patch_state")).toBe(false);

    fireEvent.keyUp(splitter, { key: "ArrowRight" });
    await settle();
    fireEvent.keyDown(splitter, { key: "Home" });
    fireEvent.blur(splitter);
    await settle();

    const saves = invokeCalls.filter((call) => call.command === "patch_state");
    expect(saves.map((call) => call.args)).toEqual([
      { patch: { recordsListWidth: RECORDS_LIST_WIDTH.default + 32 } },
      { patch: { recordsListWidth: RECORDS_LIST_WIDTH.min } },
    ]);
    expect(splitter.getAttribute("aria-valuenow")).toBe(String(RECORDS_LIST_WIDTH.min));
  });

  it("narrows the list when the window narrows, saving nothing", async () => {
    await mount();
    expect(shownListWidth()).toBe(`${RECORDS_LIST_WIDTH.default}px`);

    await act(async () => {
      box.shellWidth = RECORDS_LIST_WIDTH.min + SPLITTER_WIDTH + RECORDS_DETAIL_MIN_WIDTH;
      for (const callback of resizeCallbacks) callback();
    });

    expect(shownListWidth()).toBe(`${RECORDS_LIST_WIDTH.min}px`);
    expect(invokeCalls.some((call) => call.command === "patch_state")).toBe(false);
  });

  it("says when the records cannot be read, without the raw error", async () => {
    pages.push(new Error("SQLITE_CORRUPT /Users/someone/.onecopy/records.sqlite3"));
    await mount();

    expect(document.body.textContent).toContain("The records could not be read.");
    expect(document.body.textContent).not.toContain("SQLITE_CORRUPT");
    expect(invokeCalls.some((call) => call.command === "log_event")).toBe(true);
  });

  it("shows the failure note when a read does not answer in time", async () => {
    vi.useFakeTimers();
    pages.push(new Promise<RecordsPage>(() => {}));
    const container = document.createElement("div");
    container.id = "root-under-test";
    document.body.append(container);
    render(<RecordsWindow initialListWidth={RECORDS_LIST_WIDTH.default} />, { container });
    await tick(0);
    expect(document.body.textContent).toContain("Loading records…");

    await tick(RECORDS_READ_TIMEOUT_MS);

    expect(document.body.textContent).toContain("The records could not be read.");
  });
});

describe("the Records window's first frame", () => {
  it("opens at the saved list width, and keeps the window above its derived minimum", async () => {
    mockCommand("records_list_width", () => 500);
    const container = document.createElement("div");
    container.id = "root-under-test";
    document.body.append(container);
    render(<RecordsWindowRoute />, { container });
    await settle();

    expect(shownListWidth()).toBe("500px");
    expect(setMinSize).toHaveBeenCalledOnce();
    const size = setMinSize.mock.calls[0]![0] as unknown as { width: number; height: number };
    expect([size.width, size.height]).toEqual([computeRecordsMinWidth(), computeRecordsMinHeight()]);
  });

  it("opens at the default width when none is saved or the saved one is out of bounds", async () => {
    mockCommand("records_list_width", () => 5000);
    const container = document.createElement("div");
    container.id = "root-under-test";
    document.body.append(container);
    render(<RecordsWindowRoute />, { container });
    await settle();
    expect(shownListWidth()).toBe(`${RECORDS_LIST_WIDTH.max}px`);
    cleanup();

    mockCommand("records_list_width", () => null);
    render(<RecordsWindowRoute />, { container: document.body.appendChild(Object.assign(document.createElement("div"), { id: "root-under-test" })) });
    await settle();
    expect(shownListWidth()).toBe(`${RECORDS_LIST_WIDTH.default}px`);
  });
});
