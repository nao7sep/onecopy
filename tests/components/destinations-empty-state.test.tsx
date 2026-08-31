// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import DestinationsTab from "../../src/components/DestinationsTab";
import { useDestinationsStore } from "../../src/state/destinations-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

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
    confirmation: null,
    activePath: null,
    dragSelection: null,
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

  it("does not treat a browser payload as an internal operation", () => {
    useDestinationsStore.setState({
      roots: ["/dest"],
      expanded: new Set(),
      activePath: "/dest",
      dragSelection: null,
    });
    const view = render(<DestinationsTab />);
    const row = view.container.querySelector<HTMLElement>("[data-tree-path='/dest']")!;

    fireEvent.drop(row, {
      dataTransfer: { types: ["application/x-onecopy-drag"] },
    });
    expect(useDestinationsStore.getState().pendingDrop).toBeNull();
  });

  it("uses Enter and double-click only to expand or collapse", async () => {
    mockCommands({ list_subdirs: () => [] });
    useDestinationsStore.setState({
      roots: ["/dest"],
      expanded: new Set(),
      activePath: "/dest",
    });
    const view = render(<DestinationsTab />);
    const tree = view.getByRole("tree");
    const row = view.container.querySelector<HTMLElement>("[data-tree-path='/dest']")!;

    await act(async () => fireEvent.keyDown(tree, { key: "Enter" }));
    expect(useDestinationsStore.getState().expanded.has("/dest")).toBe(true);

    await act(async () => fireEvent.doubleClick(row));
    expect(useDestinationsStore.getState().expanded.has("/dest")).toBe(false);
    expect(
      invokeCalls.filter(({ command }) => command === "move_items_out"),
    ).toHaveLength(0);
  });
});
