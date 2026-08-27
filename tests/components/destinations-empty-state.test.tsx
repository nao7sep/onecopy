// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import DestinationsTab from "../../src/components/DestinationsTab";
import { useDestinationsStore } from "../../src/state/destinations-store";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useDestinationsStore.setState({
    roots: [],
    children: {},
    listing: {},
    expanded: new Set(),
    emptiness: {},
    message: "",
    result: null,
    activePath: null,
    pendingDrop: null,
  });
});

afterEach(() => cleanup());

describe("destination folder states", () => {
  it("keeps the empty tree reachable and explains how to populate it", () => {
    render(<DestinationsTab />);

    const tree = document.querySelector<HTMLElement>("[role='tree']");
    expect(tree?.tabIndex).toBe(0);
    expect(tree?.textContent).toContain("Add a destination root");
  });

  it("distinguishes reading, failure, and a loaded empty folder", async () => {
    let reject!: (error: Error) => void;
    mockCommands({
      list_subdirs: () => new Promise((_resolve, fail) => (reject = fail)),
    });
    useDestinationsStore.setState({
      roots: ["/dest"],
      expanded: new Set(["/dest"]),
      activePath: "/dest",
    });
    const view = render(<DestinationsTab />);
    expect(view.container.textContent).toContain("Reading folders…");

    await act(async () => reject(new Error("offline")));
    expect(view.container.textContent).toContain("Couldn’t read this folder.");
    expect(view.container.textContent).not.toContain("No subfolders");

    mockCommands({ list_subdirs: () => [] });
    await act(async () => useDestinationsStore.getState().refreshNode("/dest"));
    expect(view.container.textContent).toContain("No subfolders");
  });

  it("keeps external files denial-only even when released over a destination row", () => {
    useDestinationsStore.setState({
      roots: ["/dest"],
      expanded: new Set(),
      activePath: "/dest",
      pendingDrop: null,
    });
    const view = render(<DestinationsTab />);
    const row = view.container.querySelector<HTMLElement>("[data-tree-path='/dest']")!;

    fireEvent.drop(row, { dataTransfer: { types: ["Files"] } });

    expect(useDestinationsStore.getState().pendingDrop).toBeNull();
    expect(view.container.textContent).not.toContain("Drop into");
  });

  it("opens the Move/Copy choice only for OneCopy's internal selection", () => {
    useDestinationsStore.setState({
      roots: ["/dest"],
      expanded: new Set(),
      activePath: "/dest",
      pendingDrop: null,
    });
    const view = render(<DestinationsTab />);
    const row = view.container.querySelector<HTMLElement>("[data-tree-path='/dest']")!;

    fireEvent.drop(row, {
      dataTransfer: { types: ["application/x-onecopy-drag"] },
    });

    expect(useDestinationsStore.getState().pendingDrop).toBe("/dest");
    expect(view.container.textContent).toContain("Drop into dest");
  });

  it("keeps ordinary text drops native inside a real editor", () => {
    useDestinationsStore.setState({
      roots: ["/dest"],
      expanded: new Set(),
      activePath: "/dest",
    });
    const view = render(<DestinationsTab />);
    fireEvent.click(view.getByText("New subfolder"));
    const input = view.getByPlaceholderText("folder name");
    const dataTransfer = { types: ["text/plain"], items: [], dropEffect: "copy" };

    const over = new Event("dragover", { bubbles: true, cancelable: true });
    Object.defineProperties(over, { dataTransfer: { value: dataTransfer } });
    input.dispatchEvent(over);
    expect(over.defaultPrevented).toBe(false);

    const drop = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperties(drop, { dataTransfer: { value: dataTransfer } });
    input.dispatchEvent(drop);
    expect(drop.defaultPrevented).toBe(false);
  });
});
