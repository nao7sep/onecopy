// @vitest-environment happy-dom

import { waitFor } from "@testing-library/react";
import { beforeEach, expect, it } from "vitest";
import { useIssuesStore, type IssueRow } from "../../src/state/issues-store";
import { installIssuesEventWiring } from "../../src/workflows/issues";
import { fireEvent, mockCommands, resetTauriMocks } from "../mocks/tauri";

type Page = { total: number; rows: IssueRow[] };
let requests: Array<{ args: unknown; resolve: (page: Page) => void; reject: (error: Error) => void }>;
let activeReads: number;
let mostActive: number;
function row(id: number): IssueRow {
  return { id, path: `/fixture/${id}`, displayPath: `/fixture/${id}`, kind: "decode-error", message: "bad file", messageKey: null,
    messageValues: null, firstSeenUtc: `2026-01-01T00:00:0${id}.000Z`,
    lastSeenUtc: "2026-01-01T00:01:00.000Z", occurrenceCount: 1 };
}
const pending = (count: number) => waitFor(() => expect(requests).toHaveLength(count));

beforeEach(async () => {
  resetTauriMocks({ keepListeners: true });
  useIssuesStore.setState(useIssuesStore.getInitialState());
  requests = [];
  activeReads = 0;
  mostActive = 0;
  mockCommands({
    get_issues: (args) => {
      activeReads += 1;
      mostActive = Math.max(mostActive, activeReads);
      return new Promise<Page>((resolve, reject) => requests.push({ args, resolve, reject }))
        .finally(() => { activeReads -= 1; });
    },
    dismiss_issue: () => null,
    log_event: () => null,
  });
  await installIssuesEventWiring();
});

it("coalesces failure bursts behind the physical read and publishes completed pages", async () => {
  const loading = useIssuesStore.getState().load();
  await pending(1);
  for (let index = 0; index < 500; index += 1) fireEvent("notification://published");
  expect(requests).toHaveLength(1);
  requests[0].resolve({ total: 1, rows: [row(1)] });
  await pending(2);
  expect(useIssuesStore.getState().rows.map((value) => value.id)).toEqual([1]);
  requests[1].resolve({ total: 2, rows: [row(1), row(2)] });
  await loading;
  expect(mostActive).toBe(1);
  expect(activeReads).toBe(0);
  expect(useIssuesStore.getState()).toMatchObject({ total: 2, loading: false, loadingMore: false, error: null });
});

it("retains Show more intent but replaces its obsolete cursor after a refresh", async () => {
  useIssuesStore.setState({ rows: [row(1)], total: 4 });
  const more = useIssuesStore.getState().loadMore();
  await pending(1);
  expect(requests[0].args).toMatchObject({ afterId: 1 });
  const fresh = useIssuesStore.getState().load();
  requests[0].resolve({ total: 4, rows: [row(2)] });
  await pending(2);
  expect(useIssuesStore.getState().rows.map((value) => value.id)).toEqual([1]);
  expect(requests[1].args).not.toHaveProperty("afterId");
  requests[1].resolve({ total: 4, rows: [row(3)] });
  await pending(3);
  expect(requests[2].args).toMatchObject({ afterId: 3, afterFirstSeenUtc: row(3).firstSeenUtc });
  requests[2].resolve({ total: 4, rows: [row(4)] });
  await Promise.all([more, fresh]);
  expect(useIssuesStore.getState().rows.map((value) => value.id)).toEqual([3, 4]);
  expect(mostActive).toBe(1);
});

it("coalesces repeated paging while the same page is held", async () => {
  useIssuesStore.setState({ rows: [row(1)], total: 4 });
  const more = useIssuesStore.getState().loadMore();
  await pending(1);
  const repeated = useIssuesStore.getState().loadMore();
  requests[0].resolve({ total: 4, rows: [row(2)] });
  await Promise.all([more, repeated]);
  expect(requests).toHaveLength(1);
  expect(useIssuesStore.getState().rows.map((value) => value.id)).toEqual([1, 2]);
});

it("runs coalesced retry debt only after a failed read settles", async () => {
  const first = useIssuesStore.getState().load();
  await pending(1);
  const retry = useIssuesStore.getState().load();
  requests[0].reject(new Error("index unavailable"));
  await pending(2);
  requests[1].resolve({ total: 1, rows: [row(2)] });
  await Promise.all([first, retry]);
  expect(mostActive).toBe(1);
  expect(useIssuesStore.getState()).toMatchObject({ rows: [row(2)], loading: false, error: null });
});

it("never publishes a read from before a completed dismissal", async () => {
  const first = useIssuesStore.getState().load();
  await pending(1);
  const published: number[][] = [];
  const stop = useIssuesStore.subscribe((state) => published.push(state.rows.map((value) => value.id)));
  const dismissal = useIssuesStore.getState().dismiss(1);
  await Promise.resolve();
  requests[0].resolve({ total: 1, rows: [row(1)] });
  await pending(2);
  expect(published.every((ids) => !ids.includes(1))).toBe(true);
  requests[1].resolve({ total: 0, rows: [] });
  await Promise.all([first, dismissal]);
  stop();
  expect(mostActive).toBe(1);
  expect(useIssuesStore.getState().total).toBe(0);
});
