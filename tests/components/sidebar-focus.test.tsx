// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import Sidebar from "../../src/components/Sidebar";
import { useItemsStore } from "../../src/state/items-store";
import { useSectionsStore } from "../../src/state/sections-store";
import { mockCommands, mockSectionItems, resetTauriMocks } from "../mocks/tauri";

const COUNTS = {
  images: [{ month: "undated", count: 2 }],
  videos: [],
  others: [],
};

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    get_item_detail: () => null,
  });
  mockSectionItems(() => []);
  useItemsStore.setState({
    selected: { kind: "image", month: "undated" },
    items: [],
    selectedItem: null,
    selectedKeys: new Set(),
    sectionMemory: {},
  });
  useSectionsStore.setState({ error: null });
});

afterEach(cleanup);

describe("sidebar-to-item-area focus", () => {
  it("keeps focus in the sidebar when a section is clicked", () => {
    const view = render(<Sidebar counts={COUNTS} />);
    const tree = view.getByRole("tree");
    fireEvent.click(view.getByText("Undated"));
    expect(document.activeElement).toBe(tree);
  });

  it("Right Arrow and Tab deliberately enter the item area", () => {
    const target = document.createElement("div");
    target.id = "main-item-area";
    target.tabIndex = 0;
    document.body.appendChild(target);
    const view = render(<Sidebar counts={COUNTS} />);
    const tree = view.getByRole("tree");
    tree.focus();

    act(() => fireEvent.keyDown(tree, { key: "ArrowRight" }));
    expect(document.activeElement).toBe(target);

    tree.focus();
    act(() => fireEvent.keyDown(tree, { key: "Tab" }));
    expect(document.activeElement).toBe(target);
    target.remove();
  });
});
