// @vitest-environment happy-dom
//
// The Trash surface. The one thing that must never be casual here is Empty:
// it is the app's only control that permanently destroys the safety net's
// contents, so it has to confirm with the exact totals it is about to
// destroy, and the command must not fire before that confirmation.

import { beforeEach, afterEach, describe, expect, it } from "vitest";
import { render, cleanup, act } from "@testing-library/react";
import TrashModal from "../../src/components/TrashModal";
import { fireEvent, invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

const ROWS = [
  { root: "/Users/nao7sep/.onecopy/trash", bytes: 5_242_880, files: 42 },
  { root: "/Volumes/HDD-1/.onecopy-trash", bytes: 0, files: 0 },
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
    expect(document.body.textContent).toContain("/Volumes/HDD-1/.onecopy-trash");
    expect(document.body.textContent).toContain("42 files");
    expect(document.body.textContent).toContain("5 MB");
  });

  it("distinguishes a failed measurement from no trash locations", async () => {
    mockCommands({
      trash_overview: () => {
        throw new Error("offline");
      },
    });
    render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});

    expect(document.body.textContent).toContain("Trash locations are unavailable.");
    expect(document.body.textContent).not.toContain("No trash locations");
  });

  it("disables Empty for a root that is already empty", async () => {
    render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});
    const buttons = [...document.querySelectorAll("button")].filter(
      (b) => b.textContent === "Empty",
    );
    expect(buttons).toHaveLength(2);
    expect(buttons[0].hasAttribute("disabled")).toBe(false);
    expect(buttons[1].hasAttribute("disabled")).toBe(true);
  });

  it("confirms with the exact totals before any deletion, then empties", async () => {
    let emptied: string | null = null;
    mockCommands({
      trash_overview: () =>
        emptied === null ? ROWS : [{ ...ROWS[0], bytes: 0, files: 0 }, ROWS[1]],
      trash_empty: (args) => {
        emptied = args.root as string;
        return { cancelled: false, failures: 0 };
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
      (b) => b.textContent === "Empty trash",
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
      (button) => button.textContent === "Empty trash",
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
      (button) => button.textContent === "Empty trash",
    )!;
    await act(async () => confirm.click());

    expect(document.body.textContent).toContain(
      "Trash was processed, but its totals couldn’t be refreshed.",
    );
    expect(document.body.textContent).not.toContain("Couldn’t empty this trash.");
  });
});

describe("reveal", () => {
  it("opens the root in the file manager without touching anything", async () => {
    const { revealItemInDir } = await import("../mocks/tauri");
    render(<TrashModal open onClose={() => {}} />);
    await act(async () => {});
    const reveal = [...document.querySelectorAll("button")].find(
      (b) => b.textContent === "Reveal",
    )!;
    await act(async () => reveal.click());
    expect(revealItemInDir).toHaveBeenCalledWith(ROWS[0].root);
    // Reveal is a LOOK, never an operation: no command fired.
    expect(invokeCalls.filter((c) => c.command === "trash_empty")).toHaveLength(0);
  });
});
