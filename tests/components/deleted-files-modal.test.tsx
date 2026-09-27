// @vitest-environment happy-dom
//
// Browse: one root's deleted files, grouped by local day and deleted item,
// searchable, and selectable from the keyboard without any row taking a tab
// stop of its own.

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act, cleanup, fireEvent as fire, render } from "@testing-library/react";
import DeletedFilesModal from "../../src/components/DeletedFilesModal";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";
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
    version: 2,
    kind: "delete",
    operation: "op",
    item: "item",
    role: "main",
    movedTo: null,
    status: "restorable",
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
