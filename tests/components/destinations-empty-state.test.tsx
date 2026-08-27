// @vitest-environment happy-dom

import { act, cleanup, render } from "@testing-library/react";
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
    activePath: null,
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
});
