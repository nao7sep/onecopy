// @vitest-environment happy-dom
//
// The Trash surface. The one thing that must never be casual here is Empty:
// it is the app's only control that permanently destroys the safety net's
// contents, so it has to confirm with the exact totals it is about to
// destroy, and the command must not fire before that confirmation.

import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { render, cleanup, act } from "@testing-library/react";
import TrashModal from "../../src/components/TrashModal";
import { fireEvent, invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

const ROWS = [
  { root: "/Users/nao7sep/Photos/.onecopy-trash", available: true, bytes: 5_242_880, files: 42, planToken: "measured-42", synced: false },
  { root: "/Volumes/HDD-1/Photos/.onecopy-trash", available: true, bytes: 0, files: 0, planToken: "measured-0", synced: false },
];

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({ trash_overview: () => ROWS });
});

afterEach(() => cleanup());

describe("the trash modal", () => {
  it("measures on open and shows every root with its size", async () => {
    render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});
    expect(document.body.textContent).toContain("/Volumes/HDD-1/Photos/.onecopy-trash");
    expect(document.body.textContent).toContain("42 files");
    expect(document.body.textContent).toContain("5 MB");
  });

  it("says that emptying a synced location deletes from the cloud too", async () => {
    mockCommands({ trash_overview: () => [{ ...ROWS[0], synced: true }, ROWS[1]] });
    const view = render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});
    await act(async () => view.getAllByRole("button", { name: "Empty" })[0].click());
    expect(document.body.textContent).toContain("also deletes these files from the cloud");
  });

  it("does not mention the cloud for a local location", async () => {
    const view = render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});
    await act(async () => view.getAllByRole("button", { name: "Empty" })[0].click());
    expect(document.body.textContent).not.toContain("from the cloud");
  });

  it("distinguishes a failed measurement from no trash locations", async () => {
    mockCommands({
      trash_overview: () => {
        throw new Error("offline");
      },
    });
    render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});

    expect(document.body.textContent).toContain("Deleted-file locations are unavailable.");
    expect(document.body.textContent).not.toContain("No deleted-file locations");
  });

  it("does not publish an older measurement into a reopened modal", async () => {
    let finishOlder: ((rows: typeof ROWS) => void) | undefined;
    let calls = 0;
    mockCommands({
      trash_overview: () => {
        calls += 1;
        if (calls === 1) {
          return new Promise<typeof ROWS>((resolve) => {
            finishOlder = resolve;
          });
        }
        return [{ root: "/new", bytes: 0, files: 0 }];
      },
    });
    const view = render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});
    view.rerender(<TrashModal open={false} onClose={() => {}} />);
    view.rerender(<TrashModal open onClose={() => {}} />);
    await act(async () => {});
    finishOlder?.(ROWS);
    await act(async () => {});

    expect(document.body.textContent).toContain("/new");
    expect(document.body.textContent).not.toContain(ROWS[0].root);
  });

  it("coalesces operation events from other windows and freezes an open Empty review", async () => {
    vi.useFakeTimers();
    try {
      let current = ROWS;
      mockCommands({ trash_overview: () => current, trash_empty: () => ({ cancelled: false, failures: 0, planChanged: true }) });
      const view = render(<TrashModal open onClose={() => {}} />);
      await act(async () => {});
      await act(async () => view.getAllByRole("button", { name: "Empty" })[0].click());
      current = [{ ...ROWS[0], files: 99, planToken: "new-token" }];
      await act(async () => {
        for (let i = 0; i < 20; i++) fireEvent("mutation://progress", { kind: "destination-move", phase: "running", filesDone: i + 1 });
        fireEvent("mutation://done", { summary: { filesCompleted: 20 } });
        await vi.advanceTimersByTimeAsync(250);
      });
      expect(invokeCalls.filter((call) => call.command === "trash_overview")).toHaveLength(2);
      expect(document.body.textContent).toContain("99 files");
      expect(document.body.textContent).toContain("42 files");
      await act(async () => view.getByRole("button", { name: "Empty deleted files" }).click());
      expect(invokeCalls.find((call) => call.command === "trash_empty")?.args.planToken).toBe("measured-42");
    } finally { vi.useRealTimers(); }
  });

  it("rejects a read invalidated by a later mutation and runs one trailing read", async () => {
    let finish: ((rows: typeof ROWS) => void) | undefined;
    let calls = 0;
    mockCommands({ trash_overview: () => ++calls === 1
      ? new Promise<typeof ROWS>((resolve) => { finish = resolve; })
      : [{ ...ROWS[0], files: 7 }] });
    render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});
    await act(async () => {
      fireEvent("mutation://error", { summary: { filesCompleted: 1 } });
      fireEvent("mutation://done", { summary: { filesCompleted: 2 } });
      finish!(ROWS);
    });
    expect(calls).toBe(2);
    expect(document.body.textContent).toContain("7 files");
    expect(document.body.textContent).not.toContain("42 files");
  });

  it("disables Empty for a root that is already empty", async () => {
    render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});
    const buttons = [...document.querySelectorAll("button")].filter(
      (b) => b.textContent === "Empty",
    );
    expect(buttons).toHaveLength(2);
    expect(buttons[0].hasAttribute("disabled")).toBe(false);
    expect(buttons[0].className).toContain("border-danger/50");
    expect(buttons[0].className.split(" ")).toContain("bg-danger-surface");
    expect(buttons[1].hasAttribute("disabled")).toBe(true);
  });

  it("confirms with the exact totals before any deletion, then empties", async () => {
    let emptied: string | null = null;
    mockCommands({
      trash_overview: () =>
        emptied === null ? ROWS : [{ ...ROWS[0], bytes: 0, files: 0 }, ROWS[1]],
      trash_empty: (args) => {
        emptied = args.root as string;
        expect(args.planToken).toBe("measured-42");
        return { cancelled: false, failures: 0, planChanged: false };
      },
    });
    render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});

    const empty = [...document.querySelectorAll("button")].find(
      (b) => b.textContent === "Empty" && !b.hasAttribute("disabled"),
    );
    await act(async () => empty!.click());

    // The confirmation names what is about to be destroyed; NOTHING has been
    // deleted yet.
    expect(document.body.textContent).toContain("42 files");
    expect(document.body.textContent).toContain("cannot be recovered");
    expect(invokeCalls.some((c) => c.command === "trash_empty")).toBe(false);

    const go = [...document.querySelectorAll("button")].find(
      (b) => b.textContent === "Empty deleted files",
    );
    await act(async () => go!.click());

    expect(emptied).toBe(ROWS[0].root);
    // And the list re-measured rather than pretending.
    expect(document.body.textContent).toContain("0 files");
  });

  it("shows stable progress and offers cooperative cancellation", async () => {
    let finish = (_value: { cancelled: boolean; failures: number }): void => {};
    mockCommands({
      trash_empty: () => new Promise((resolve) => {
        finish = resolve;
      }),
      trash_empty_cancel: () => true,
    });
    render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});
    const empty = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Empty" && !button.hasAttribute("disabled"),
    )!;
    await act(async () => empty.click());
    const confirm = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Empty deleted files",
    )!;
    await act(async () => confirm.click());

    fireEvent("trash://progress", {
      root: ROWS[0].root,
      progress: {
        done: 12,
        total: 42,
        bytesDone: 1_048_576,
        bytesTotal: 5_242_880,
        failures: 1,
      },
    });
    await act(async () => {});
    expect(document.body.textContent).toContain("Removing — 12/42 · 1 MB/5 MB · 1 failed");

    const cancel = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Cancel",
    )!;
    await act(async () => cancel.click());
    expect(invokeCalls).toContainEqual({ command: "trash_empty_cancel", args: {} });
    expect(document.body.textContent).toContain("Cancelling…");

    finish({ cancelled: true, failures: 0 });
    await act(async () => {});
  });

  it("does not misreport a completed operation when remeasurement fails", async () => {
    let measurements = 0;
    mockCommands({
      trash_overview: () => {
        measurements += 1;
        if (measurements > 1) throw new Error("volume left");
        return ROWS;
      },
      trash_empty: () => ({ cancelled: false, failures: 0 }),
    });
    render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});
    const empty = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Empty" && !button.hasAttribute("disabled"),
    )!;
    await act(async () => empty.click());
    const confirm = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Empty deleted files",
    )!;
    await act(async () => confirm.click());

    expect(document.body.textContent).toContain(
      "Deleted files were processed, but their totals couldn’t be refreshed.",
    );
    expect(document.body.textContent).not.toContain("Couldn’t empty these deleted files.");
  });
});

describe("reveal", () => {
  it("opens the root in the file manager without touching anything", async () => {
    mockCommands({ trash_reveal: () => undefined });
    render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});
    const reveal = [...document.querySelectorAll("button")].find(
      (b) => b.textContent === "Reveal",
    )!;
    await act(async () => reveal.click());
    expect(invokeCalls).toContainEqual({
      command: "trash_reveal",
      args: { root: ROWS[0].root },
    });
    // Reveal never starts a destructive operation.
    expect(invokeCalls.filter((c) => c.command === "trash_empty")).toHaveLength(0);
  });

  it("removes nothing and shows the new totals when the location changed after review", async () => {
    let measured = 0;
    mockCommands({
      trash_overview: () => {
        measured += 1;
        return measured === 1
          ? ROWS
          : [{ ...ROWS[0], files: 1_020, bytes: 9_000_000, planToken: "measured-1020" }, ROWS[1]];
      },
      trash_empty: () => ({ cancelled: false, failures: 0, planChanged: true }),
    });
    render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});
    const empty = [...document.querySelectorAll("button")].find(
      (b) => b.textContent === "Empty" && !b.hasAttribute("disabled"),
    );
    await act(async () => empty!.click());
    const go = [...document.querySelectorAll("button")].find(
      (b) => b.textContent === "Empty deleted files",
    );
    await act(async () => go!.click());

    expect(invokeCalls.find((c) => c.command === "trash_empty")?.args).toEqual({
      root: ROWS[0].root,
      planToken: "measured-42",
    });
    expect(document.body.textContent).toContain("nothing was removed");
    expect(document.body.textContent).toContain("1,020 files");
  });
});

describe("browsing a root", () => {
  it("opens Browse for an available root and refuses an unavailable one", async () => {
    mockCommands({
      trash_overview: () => [
        { ...ROWS[0] },
        { ...ROWS[0], root: "/Volumes/Gone/.onecopy-trash", available: false },
      ],
      trash_entries: () => ({ entries: [], unrecordedFiles: 0, malformedLines: 0, newerLines: 0 }),
    });
    render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});
    expect(document.body.textContent).toContain("Unavailable — this folder is not connected right now");
    const browse = [...document.querySelectorAll("button")].filter((b) => b.textContent === "Browse…");
    expect(browse).toHaveLength(2);
    expect(browse[1].hasAttribute("disabled")).toBe(true);
    await act(async () => browse[0].click());
    expect(invokeCalls.some((c) => c.command === "trash_entries" && c.args.root === ROWS[0].root)).toBe(true);
    expect(document.body.textContent).toContain("Browse deleted files");
  });
});
