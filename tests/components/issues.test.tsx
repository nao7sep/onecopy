// @vitest-environment happy-dom
//
// One current-run diagnostic inbox, without work-admission controls.

import { beforeEach, afterEach, describe, expect, it } from "vitest";
import { render, cleanup, act } from "@testing-library/react";
import IssuesModal from "../../src/components/IssuesModal";
import { useIssuesStore, type IssueRow } from "../../src/state/issues-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

function row(id: number, over: Partial<IssueRow> = {}): IssueRow {
  return {
    id,
    path: `/vol/photos/IMG_${id}.jpg`,
    kind: "decode-error",
    message: "could not decode",
    firstSeenUtc: `2026-08-0${id}T00:00:00.000Z`,
    lastSeenUtc: "2026-08-16T00:00:00.000Z",
    occurrenceCount: 1,
    ...over,
  };
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useIssuesStore.setState({
    total: 0,
    rows: [],
    loading: false,
    error: null,
  });
});

afterEach(() => cleanup());

describe("the issues modal", () => {
  it("stays entirely absent while closed", () => {
    const view = render(<IssuesModal open={false} onClose={() => {}} />);
    expect(view.container.innerHTML).toBe("");
  });

  it("loads on open and renders oldest-first rows with their stamps", async () => {
    mockCommands({
      get_issues: () => ({ total: 2, rows: [row(1), row(2)] }),
    });
    render(<IssuesModal open onClose={() => {}} />);
    await act(async () => {});

    const items = document.querySelectorAll("li");
    expect(items).toHaveLength(2);
    // The backend orders oldest first; the list must not re-sort it.
    expect(items[0].textContent).toContain("IMG_1");
    expect(items[0].textContent).not.toContain("Action needed");
    expect(items[0].textContent).not.toContain("decode-error");
  });

  it("distinguishes an unavailable list from no issues", async () => {
    mockCommands({
      get_issues: () => {
        throw new Error("offline");
      },
    });
    render(<IssuesModal open onClose={() => {}} />);
    await act(async () => {});

    expect(document.body.textContent).toContain("Issues are unavailable.");
    expect(document.body.textContent).not.toContain("No issues");
  });

  it("dismisses one row through the command and reloads", async () => {
    let rows = [row(1), row(2)];
    mockCommands({
      get_issues: () => ({ total: rows.length, rows }),
      dismiss_issue: (args) => {
        rows = rows.filter((r) => r.id !== args.id);
        return null;
      },
    });
    render(<IssuesModal open onClose={() => {}} />);
    await act(async () => {});

    await act(async () => {
      (document.querySelector('[aria-label="Dismiss"]') as HTMLElement).click();
    });

    expect(invokeCalls.some((c) => c.command === "dismiss_issue")).toBe(true);
    expect(document.querySelectorAll("li")).toHaveLength(1);
  });

  it("dismisses everything at once", async () => {
    let rows = [row(1), row(2), row(3)];
    mockCommands({
      get_issues: () => ({ total: rows.length, rows }),
      dismiss_all_issues: () => {
        rows = [];
        return null;
      },
    });
    render(<IssuesModal open onClose={() => {}} />);
    await act(async () => {});

    const all = [...document.querySelectorAll("button")].find(
      (b) => b.textContent === "Dismiss all",
    );
    await act(async () => all!.click());

    expect(document.body.textContent).toContain("No issues");
  });

  it("shows failed actions and conditions together without tabs or retry controls", async () => {
    mockCommands({ get_issues: () => ({ total: 501, rows: [row(1), row(2, {
      kind: "open-failed", message: "Couldn’t open the file.", occurrenceCount: 2,
    })] }) });
    render(<IssuesModal open onClose={() => {}} />);
    await act(async () => {});
    expect(document.querySelector('[role="tablist"]')).toBeNull();
    expect(document.body.textContent).toContain("Couldn’t open the file.");
    expect(document.body.textContent).toContain("×2");
    expect(document.body.textContent).toContain("Showing the oldest 2 of 501");
    expect(document.body.textContent).not.toContain("Retry");
    expect(invokeCalls.map((call) => call.command)).toEqual(["get_issues"]);
  });

  it("does not restore stale rows over a newer refresh", async () => {
    let resolveOld!: (value: { total: number; rows: IssueRow[] }) => void;
    let calls = 0;
    mockCommands({ get_issues: () => ++calls === 1
      ? new Promise((resolve) => { resolveOld = resolve; }) : { total: 0, rows: [] } });
    const old = useIssuesStore.getState().load();
    await useIssuesStore.getState().load();
    resolveOld({ total: 1, rows: [row(1)] });
    await old;
    expect(useIssuesStore.getState().rows).toEqual([]);
    expect(useIssuesStore.getState().total).toBe(0);
  });
});
