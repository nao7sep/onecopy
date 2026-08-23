// @vitest-environment happy-dom
//
// The Trash surface. The one thing that must never be casual here is Empty:
// it is the app's only control that permanently destroys the safety net's
// contents, so it has to confirm with the exact totals it is about to
// destroy, and the command must not fire before that confirmation.

import { beforeEach, afterEach, describe, expect, it } from "vitest";
import { render, cleanup, act } from "@testing-library/react";
import TrashModal from "../../src/components/TrashModal";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

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
        return null;
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
