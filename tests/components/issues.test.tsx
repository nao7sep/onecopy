// @vitest-environment happy-dom
//
// Active keeps the status-bar count and safe recovery controls; Recent keeps
// bounded notification history without becoming a replay surface.

import { beforeEach, afterEach, describe, expect, it } from "vitest";
import { render, cleanup, act, fireEvent } from "@testing-library/react";
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
    recovery: null,
    ...over,
  };
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({ get_recent_notifications: () => ({ total: 0, rows: [] }) });
  useIssuesStore.setState({
    total: 0,
    rows: [],
    loading: false,
    error: null,
    recentTotal: 0,
    recentRows: [],
    recentLoading: false,
    recentError: null,
    view: "active",
    open: false,
  });
});

afterEach(() => cleanup());

describe("the issues modal", () => {
  it("stays entirely absent while closed", () => {
    const view = render(<IssuesModal />);
    expect(view.container.innerHTML).toBe("");
  });

  it("loads on open and renders oldest-first rows with their stamps", async () => {
    mockCommands({
      get_issues: () => ({ total: 2, rows: [row(1), row(2)] }),
    });
    render(<IssuesModal />);
    await act(async () => useIssuesStore.getState().setOpen(true));

    const items = document.querySelectorAll("li");
    expect(items).toHaveLength(2);
    // The backend orders oldest first; the list must not re-sort it.
    expect(items[0].textContent).toContain("IMG_1");
    expect(items[0].textContent).toContain("Action needed");
    expect(items[0].textContent).not.toContain("decode-error");
  });

  it("distinguishes an unavailable list from no issues", async () => {
    mockCommands({
      get_issues: () => {
        throw new Error("offline");
      },
    });
    render(<IssuesModal />);
    await act(async () => useIssuesStore.getState().setOpen(true));

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
    render(<IssuesModal />);
    await act(async () => useIssuesStore.getState().setOpen(true));

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
    render(<IssuesModal />);
    await act(async () => useIssuesStore.getState().setOpen(true));

    const all = [...document.querySelectorAll("button")].find(
      (b) => b.textContent === "Dismiss all",
    );
    await act(async () => all!.click());

    expect(document.body.textContent).toContain("No active issues");
  });

  it("retries only backend-authorized rows and leaves them visible as queued", async () => {
    let rows = [
      row(1, { recovery: { action: "retry", label: "Retry", status: "available" } }),
      row(2, { kind: "delete-error" }),
    ];
    mockCommands({
      get_issues: () => ({ total: rows.length, rows }),
      retry_issue: (args) => {
        rows = rows.map((item) =>
          item.id === args.id
            ? {
                ...item,
                recovery: {
                  action: "retry" as const,
                  label: "Retry",
                  status: "queued" as const,
                },
              }
            : item,
        );
        return true;
      },
    });
    render(<IssuesModal />);
    await act(async () => useIssuesStore.getState().setOpen(true));

    const retry = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Retry",
    );
    await act(async () => retry!.click());

    expect(invokeCalls.some((call) => call.command === "retry_issue")).toBe(true);
    expect(document.body.textContent).toContain("Queued");
    expect(document.querySelectorAll("li")).toHaveLength(2);
  });

  it("offers retry all only while at least one safe retry is available", async () => {
    let rows = [
      row(1, { recovery: { action: "retry", label: "Retry", status: "available" } }),
      row(2, { recovery: { action: "retry", label: "Retry", status: "queued" } }),
    ];
    mockCommands({
      get_issues: () => ({ total: rows.length, rows }),
      retry_all_issues: () => {
        rows = rows.map((item) => ({
          ...item,
          recovery: {
            action: "retry" as const,
            label: "Retry",
            status: "queued" as const,
          },
        }));
        return 1;
      },
    });
    render(<IssuesModal />);
    await act(async () => useIssuesStore.getState().setOpen(true));

    const retryAll = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Retry all",
    );
    await act(async () => retryAll!.click());

    expect(invokeCalls.some((call) => call.command === "retry_all_issues")).toBe(true);
    expect(document.body.textContent).not.toContain("Retry all");
  });

  it("runs a backend-authored filesystem recheck without folding it into retry all", async () => {
    let rows = [
      row(1, {
        kind: "read-error",
        recovery: { action: "recheck", label: "Recheck", status: "available" },
      }),
    ];
    let release!: () => void;
    mockCommands({
      get_issues: () => ({ total: rows.length, rows }),
      recheck_issue: () =>
        new Promise<{ status: "started" }>((resolve) => {
          release = () => {
            rows = [];
            resolve({ status: "started" });
          };
        }),
    });
    render(<IssuesModal />);
    await act(async () => useIssuesStore.getState().setOpen(true));

    const recheck = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Recheck",
    );
    act(() => recheck!.click());
    expect(document.body.textContent).toContain("Running");
    expect(document.body.textContent).not.toContain("Retry all");

    await act(async () => release());
    expect(invokeCalls.some((call) => call.command === "recheck_issue")).toBe(true);
    expect(document.body.textContent).toContain("No active issues");
  });

  it("keeps restart-persistent notification history separate from active conditions", async () => {
    mockCommands({
      get_issues: () => ({ total: 0, rows: [] }),
      get_recent_notifications: () => ({
        total: 1,
        rows: [
          {
            id: 9,
            kind: "open-failed",
            path: null,
            level: "error",
            presentation: "persistent",
            message: "Couldn’t open the selected file.",
            firstSeenUtc: "2026-08-30T00:00:00.000Z",
            lastSeenUtc: "2026-08-31T00:00:00.000Z",
            occurrenceCount: 2,
          },
        ],
      }),
    });
    render(<IssuesModal />);
    await act(async () => useIssuesStore.getState().setOpen(true));
    await act(async () => useIssuesStore.getState().setView("recent"));

    expect(document.body.textContent).toContain("Recent (1)");
    expect(document.body.textContent).toContain("Couldn’t open the selected file.");
    expect(document.body.textContent).toContain("×2");
    expect(document.body.textContent).not.toContain("Dismiss all");

    const recentTab = document.getElementById("issues-tab-recent") as HTMLElement;
    recentTab.focus();
    fireEvent.keyDown(recentTab, { key: "ArrowLeft" });
    expect(useIssuesStore.getState().view).toBe("active");
    expect(document.activeElement?.id).toBe("issues-tab-active");
  });
});
