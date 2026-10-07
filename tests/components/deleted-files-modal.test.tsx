// @vitest-environment happy-dom
//
// Browse: one root's deleted files, grouped by local day and deleted item,
// searchable, and selectable from the keyboard without any row taking a tab
// stop of its own.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent as fire, render } from "@testing-library/react";
import DeletedFilesModal from "../../src/components/DeletedFilesModal";
import { fireEvent, invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";
import type { TrashEntry, TrashListing } from "../../src/models/deletedFiles";

const LOCATION = "/Photos/.onecopy-trash";

function entry(overrides: Partial<TrashEntry> & { id: string }): TrashEntry {
  const storedName = overrides.id.split("/").pop()!;
  return {
    day: "20260927-utc",
    storedName,
    storedPath: `${LOCATION}/${overrides.id}`,
    originalRelative: `trips/${storedName}`,
    deletedAtUtc: "2026-09-27T10:00:00.000Z",
    size: 1024,
    mtimeMs: 1,
    group: `item:op:${overrides.item ?? "item"}`,
    kind: "delete",
    operation: "op",
    item: "item",
    role: "main",
    movedTo: null,
    status: "restorable",
    mainRestoredAs: null,
    ...overrides,
  };
}

const LISTING: TrashListing = {
  entries: [
    entry({ id: "20260927-utc/beach.jpg" }),
    entry({ id: "20260927-utc/beach.xmp", role: "companion" }),
    entry({ id: "20260927-utc/notes.txt", item: "other", originalRelative: "docs/notes.txt", status: "changed" }),
  ],
  unrecordedFiles: 2,
  malformedLines: 0,
  newerLines: 0,
};

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({ trash_entries: () => LISTING });
});

afterEach(() => cleanup());

function list(): HTMLElement {
  return document.querySelector('[role="listbox"]') as HTMLElement;
}

describe("Deleted files browse", () => {
  it("refreshes after another window deletes files while retaining selection and the visible row", async () => {
    vi.useFakeTimers();
    try {
      let entries = Array.from({ length: 40 }, (_, i) => entry({ id: `20260927-utc/item-${String(i).padStart(2, "0")}.jpg`, item: String(i), group: `group-${i}` }));
      mockCommands({ trash_entries: () => ({ ...LISTING, entries }) });
      render(<DeletedFilesModal location={LOCATION} onClose={() => {}} />);
      await act(async () => {});
      const box = list();
      await act(async () => { fire.keyDown(box, { key: "ArrowDown" }); });
      await act(async () => { fire.keyDown(box, { key: " " }); });
      const active = box.getAttribute("aria-activedescendant");
      await act(async () => { box.scrollTop = 480; fire.scroll(box); });
      entries = [entry({ id: "20261001-utc/new.jpg", day: "20261001-utc", deletedAtUtc: "2026-10-01T00:00:00Z", item: "new", group: "new" }), ...entries];
      await act(async () => {
        fireEvent("mutation://done", { summary: { filesCompleted: 1 } });
        await vi.advanceTimersByTimeAsync(250);
      });
      expect(box.getAttribute("aria-activedescendant")).toBe(active);
      expect(box.scrollTop).toBe(576); // the new day and group precede the old anchor
      expect(document.body.textContent).toContain("1 file selected");
      expect(invokeCalls.filter((call) => call.command === "trash_entries")).toHaveLength(2);
    } finally { vi.useRealTimers(); }
  });

  it("keeps the last good listing and shows a live refresh failure", async () => {
    vi.useFakeTimers();
    try {
      let failed = false;
      mockCommands({ trash_entries: () => { if (failed) throw new Error("offline"); return LISTING; } });
      render(<DeletedFilesModal location={LOCATION} onClose={() => {}} />);
      await act(async () => {});
      failed = true;
      await act(async () => { fireEvent("mutation://error", {}); await vi.advanceTimersByTimeAsync(250); });
      expect(document.body.textContent).toContain("beach.jpg");
      expect(document.body.textContent).toContain("Couldn’t read the deleted files");
      failed = false;
      await act(async () => { fireEvent("mutation://done", { summary: {} }); await vi.advanceTimersByTimeAsync(250); });
      expect(document.body.textContent).not.toContain("Couldn’t read the deleted files");
    } finally { vi.useRealTimers(); }
  });

  it("lists one row per deleted item with its counts and reasons", async () => {
    render(<DeletedFilesModal location={LOCATION} onClose={() => {}} />);
    await act(async () => {});
    const options = [...document.querySelectorAll('[role="option"]')];
    expect(options).toHaveLength(2);
    expect(document.body.textContent).toContain("beach.jpg");
    expect(document.body.textContent).toContain("1 copy · 1 companion");
    expect(document.body.textContent).toContain("Changed since it was deleted");
    expect(document.body.textContent).toContain("2 files here have no record");
    const changed = options.find((option) => option.textContent?.includes("notes.txt"))!;
    expect(changed.getAttribute("aria-disabled")).toBe("true");
  });

  it("says how many record lines a newer OneCopy wrote and left as they are", async () => {
    mockCommands({ trash_entries: () => ({ ...LISTING, newerLines: 3 }) });
    render(<DeletedFilesModal location={LOCATION} onClose={() => {}} />);
    await act(async () => {});
    expect(document.body.textContent).toContain(
      "3 record lines were written by a newer version of OneCopy and were left as they are.",
    );
  });

  it("searches by name or original folder", async () => {
    render(<DeletedFilesModal location={LOCATION} onClose={() => {}} />);
    await act(async () => {});
    const search = document.querySelector('input[type="search"]') as HTMLInputElement;
    await act(async () => fire.change(search, { target: { value: "DOCS" } }));
    expect(document.querySelectorAll('[role="option"]')).toHaveLength(1);
    await act(async () => fire.change(search, { target: { value: "nothing" } }));
    expect(document.body.textContent).toContain("No deleted files match this search.");
  });

  it("selects a whole item with Space and single files after expanding it", async () => {
    render(<DeletedFilesModal location={LOCATION} onClose={() => {}} />);
    await act(async () => {});
    const listbox = list();
    expect(listbox.getAttribute("aria-multiselectable")).toBe("true");
    expect(listbox.querySelectorAll("[tabindex]")).toHaveLength(0);
    await act(async () => fire.focus(listbox));
    await act(async () => fire.keyDown(listbox, { key: " " }));
    expect(document.body.textContent).toContain("2 files selected");
    await act(async () => fire.keyDown(listbox, { key: "ArrowRight" }));
    expect(document.querySelectorAll('[role="option"]')).toHaveLength(4);
    await act(async () => fire.keyDown(listbox, { key: "ArrowDown" }));
    await act(async () => fire.keyDown(listbox, { key: "ArrowDown" }));
    await act(async () => fire.keyDown(listbox, { key: " " }));
    expect(document.body.textContent).toContain("1 file selected");
    // The changed file cannot be selected.
    await act(async () => fire.keyDown(listbox, { key: "End" }));
    await act(async () => fire.keyDown(listbox, { key: " " }));
    expect(document.body.textContent).toContain("1 file selected");
  });

  it("drops a listing that arrives after the modal closed", async () => {
    let finish: ((listing: TrashListing) => void) | undefined;
    mockCommands({
      trash_entries: () => new Promise<TrashListing>((resolve) => (finish = resolve)),
    });
    const view = render(<DeletedFilesModal location={LOCATION} onClose={() => {}} />);
    view.unmount();
    finish?.(LISTING);
    await act(async () => {});
    expect(document.body.textContent).not.toContain("beach.jpg");
  });

  it("says when the location cannot be read", async () => {
    mockCommands({
      trash_entries: () => {
        throw new Error("deleted-file storage is not a real directory");
      },
    });
    render(<DeletedFilesModal location={LOCATION} onClose={() => {}} />);
    await act(async () => {});
    expect(document.body.textContent).toContain("Couldn’t read the deleted files in this location.");
  });
});

describe("Restore", () => {
  const outcome = (overrides: Record<string, unknown>) => ({
    cancelled: false,
    error: null,
    restored: [],
    alreadyPresent: 0,
    failed: 0,
    unknown: 0,
    unstarted: 0,
    filesTotal: 2,
    planToken: "token-1",
    requiresReview: false,
    planChanged: false,
    review: null,
    ...overrides,
  });

  async function selectFirstItem() {
    render(<DeletedFilesModal location={LOCATION} onClose={() => {}} />);
    await act(async () => {});
    const listbox = list();
    await act(async () => fire.focus(listbox));
    await act(async () => fire.keyDown(listbox, { key: " " }));
  }

  function button(label: string): HTMLButtonElement {
    return [...document.querySelectorAll("button")].find((b) => b.textContent === label) as HTMLButtonElement;
  }

  it("restores a clean selection at once and shows the receipt", async () => {
    const calls: Array<Record<string, unknown>> = [];
    mockCommands({
      trash_entries: () => LISTING,
      trash_restore: (args) => {
        calls.push(args);
        return outcome({ restored: ["/Photos/trips/beach.jpg", "/Photos/trips/beach.xmp"] });
      },
    });
    await selectFirstItem();
    expect(button("Restore").disabled).toBe(false);
    await act(async () => button("Restore").click());
    expect(calls).toEqual([
      {
        root: LOCATION,
        entries: ["20260927-utc/beach.jpg", "20260927-utc/beach.xmp"],
        planToken: null,
      },
    ]);
    expect(document.body.textContent).toContain("Restore complete — 2 restored");
    expect(document.body.textContent).toContain("0 files selected");
  });

  it("asks for review only when needed and confirms with the reviewed token", async () => {
    const calls: Array<Record<string, unknown>> = [];
    mockCommands({
      trash_entries: () => LISTING,
      trash_restore: (args) => {
        calls.push(args);
        return args.planToken === null
          ? outcome({
              requiresReview: true,
              review: {
                files: [
                  { id: "20260927-utc/beach.jpg", original: "trips/beach.jpg", target: "trips/beach 2.jpg", renamed: true, skip: null, mainRestoredAs: null },
                  { id: "20260927-utc/beach.xmp", original: "trips/beach.xmp", target: "trips/beach 2.xmp", renamed: true, skip: null, mainRestoredAs: null },
                ],
                folders: ["trips"],
                companionsLeft: [],
              },
            })
          : outcome({ restored: ["/Photos/trips/beach 2.jpg", "/Photos/trips/beach 2.xmp"] });
      },
    });
    await selectFirstItem();
    await act(async () => button("Restore").click());
    expect(document.body.textContent).toContain("Review restore");
    expect(document.body.textContent).toContain("Original name is taken — restored as trips/beach 2.jpg");
    expect(document.body.textContent).toContain("These folders will be recreated:");
    await act(async () => button("Rename and Restore").click());
    expect(calls[1]).toEqual({ ...calls[0], planToken: "token-1" });
    expect(document.body.textContent).not.toContain("Review restore");
    expect(document.body.textContent).toContain("2 restored");
  });

  it("says when a companion comes back without the name its restored main now has", async () => {
    mockCommands({
      trash_entries: () => LISTING,
      trash_restore: () =>
        outcome({
          requiresReview: true,
          review: {
            files: [
              { id: "20260927-utc/beach.xmp", original: "trips/beach.xmp", target: "trips/beach.xmp", renamed: false, skip: null, mainRestoredAs: "trips/beach 2.jpg" },
            ],
            folders: [],
            companionsLeft: [],
          },
        }),
    });
    await selectFirstItem();
    await act(async () => button("Restore").click());
    expect(document.body.textContent).toContain("Restored to trips/beach.xmp");
    expect(document.body.textContent).toContain(
      "Its main file was restored earlier as trips/beach 2.jpg — this companion keeps its original name and will not pair with it",
    );
  });

  it("cancelling the review restores nothing", async () => {
    const calls: Array<Record<string, unknown>> = [];
    mockCommands({
      trash_entries: () => LISTING,
      trash_restore: (args) => {
        calls.push(args);
        return outcome({
          requiresReview: true,
          review: { files: [{ id: "x", original: "x.jpg", target: null, renamed: false, skip: "already-there", mainRestoredAs: null }], folders: [], companionsLeft: [] },
        });
      },
    });
    await selectFirstItem();
    await act(async () => button("Restore").click());
    expect(document.body.textContent).toContain("Already at its original location — stays in Deleted files");
    const dialogs = document.querySelectorAll('[role="dialog"]');
    const review = dialogs[dialogs.length - 1];
    const primary = [...review.querySelectorAll("button")].find((b) => b.textContent === "Restore")!;
    expect(primary.disabled).toBe(true);
    const cancel = [...review.querySelectorAll("button")].find((b) => b.textContent === "Cancel")!;
    await act(async () => cancel.click());
    expect(calls).toHaveLength(1);
    expect(document.body.textContent).not.toContain("Review restore");
  });
});
