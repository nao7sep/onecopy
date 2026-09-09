// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import Sidebar from "../../src/components/Sidebar";
import { useItemsStore } from "../../src/state/items-store";
import { useSectionsStore } from "../../src/state/sections-store";
import { mockCommands, mockSectionItems, resetTauriMocks } from "../mocks/tauri";
import { pushModal, resetModalStack } from "../../src/utils/modalStack";
import { useComparisonStore } from "../../src/state/comparison-store";
import { useQuickViewStore } from "../../src/state/quick-view-store";

const COUNTS = {
  images: [{ month: "undated", count: 2 }],
  videos: [],
  others: [],
};

beforeEach(() => {
  resetTauriMocks();
  resetModalStack();
  useComparisonStore.setState({ open: false });
  useQuickViewStore.setState({ session: null });
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

afterEach(() => { cleanup(); resetModalStack(); });

describe("sidebar-to-item-area focus", () => {
  it("navigates category and year rows independently from the open month", () => {
    const view = render(<Sidebar counts={{ ...COUNTS, images: [{ month: "2026-01", count: 2 }, ...COUNTS.images] }} />);
    const tree = view.getByRole("tree");
    tree.focus();
    const active = () => document.getElementById(tree.getAttribute("aria-activedescendant")!)?.getAttribute("data-row-key");
    fireEvent.keyDown(tree, { key: "Home" });
    expect(active()).toBe("kind:image");
    fireEvent.keyDown(tree, { key: "ArrowDown" });
    expect(active()).toBe("year:image:2026");
    fireEvent.keyDown(tree, { key: "ArrowRight" });
    expect(active()).toBe("year:image:2026");
    fireEvent.keyDown(tree, { key: "ArrowRight" });
    expect(active()).toBe("month:image:2026-01");
    fireEvent.keyDown(tree, { key: "ArrowLeft" });
    expect(active()).toBe("year:image:2026");
    fireEvent.keyDown(tree, { key: "ArrowLeft" });
    expect(view.queryByText("2026-01")).toBeNull();
    expect(useItemsStore.getState().selected?.month).toBe("2026-01");
    expect(document.activeElement).toBe(tree);
  });

  it.each(["composition", "legacy composition", "modal", "comparison", "modifier"])("does not navigate behind %s input ownership", (owner) => {
    const view = render(<Sidebar counts={COUNTS} />);
    const tree = view.getByRole("tree");
    tree.focus();
    const before = tree.getAttribute("aria-activedescendant");
    if (owner === "modal") pushModal({});
    if (owner === "comparison") useComparisonStore.setState({ open: true });
    fireEvent.keyDown(tree, { key: "Home", isComposing: owner === "composition", keyCode: owner === "legacy composition" ? 229 : 0, ctrlKey: owner === "modifier" });
    expect(tree.getAttribute("aria-activedescendant")).toBe(before);
  });

  it("restores an externally chosen month without stealing another control's focus", () => {
    const view = render(<Sidebar counts={{ ...COUNTS, images: [{ month: "2026-01", count: 2 }, ...COUNTS.images] }} />);
    const field = document.createElement("input");
    document.body.appendChild(field);
    field.focus();
    act(() => useItemsStore.setState({ selected: { kind: "image", month: "2026-01" } }));
    const tree = view.getByRole("tree");
    expect(document.getElementById(tree.getAttribute("aria-activedescendant")!)?.getAttribute("data-row-key"))
      .toBe("month:image:2026-01");
    expect(document.activeElement).toBe(field);
    field.remove();
  });

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
