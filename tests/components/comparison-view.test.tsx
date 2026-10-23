// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ComparisonView from "../../src/components/ComparisonView";
import { useComparisonStore } from "../../src/state/comparison-store";
import { resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks();
  useComparisonStore.setState({
    open: true,
    members: [0, 1].map((index) => ({
      hash: `h${index}`, fileName: `photo-${index}.jpg`, width: 4000,
      height: 3000, byteSize: 1000, sharpness: null, faceScore: null,
      copyCount: 1, hasThumb: true,
    })),
    page: 0, maximumImages: 4, displayCount: 1, displayAspects: [16 / 9],
    capacities: [2], portraitDominant: false, spreadCount: 0,
    selected: new Set(), anchor: "h0", anchors: new Set(["h0"]),
    rangeOrigin: "h0", rangeBase: new Set(), busy: false, message: null,
    pendingAction: null, failure: null,
  });
});

afterEach(cleanup);

describe("Comparison view and decision store composition", () => {
  it("uses the same measured display shape for card layout and arrow navigation", () => {
    const view = render(<ComparisonView onRevealTrash={vi.fn()} />);
    const grid = view.getByRole("listbox");
    expect(grid.style.gridTemplateColumns).toBe("repeat(2, minmax(0, 1fr))");
    act(() => useComparisonStore.getState().setDisplayAspect(0, 9 / 16));
    expect(grid.style.gridTemplateColumns).toBe("repeat(1, minmax(0, 1fr))");
    fireEvent.keyDown(grid, { key: "ArrowDown" });
    expect(useComparisonStore.getState().anchor).toBe("h1");
  });
  it("renders Keep and unkeep immediately without an unrelated update", () => {
    const view = render(<ComparisonView onRevealTrash={vi.fn()} />);
    expect(view.queryByRole("button", { name: "Not similar" })).toBeNull();
    const card = view.getByRole("option", { name: "Key 1: photo-1.jpg" });

    fireEvent.click(view.getByRole("button", { name: "Keep photo-1.jpg" }));

    expect(document.activeElement?.id).toBe("comparison-item-area");
    expect(card.getAttribute("aria-selected")).toBe("true");
    expect(view.getByText(/1 marked to keep/)).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "Remove keep mark from photo-1.jpg" }));
    expect(card.getAttribute("aria-selected")).toBe("false");
    expect(view.getByText(/0 marked to keep/)).toBeTruthy();
  });

  it("renders inspection activation without importing keep intent", () => {
    const view = render(<ComparisonView onRevealTrash={vi.fn()} />);
    const first = view.getByRole("option", { name: "Key 0: photo-0.jpg" });
    const second = view.getByRole("option", { name: "Key 1: photo-1.jpg" });

    fireEvent.click(second);

    expect(first.classList.contains("ring-2")).toBe(false);
    expect(second.classList.contains("ring-2")).toBe(true);
    expect(second.getAttribute("aria-selected")).toBe("false");
  });
});

it("contains Tab in Comparison and restores its opener after closing", () => {
  const opener = document.createElement("button");
  document.body.append(opener);
  opener.focus();
  const view = render(<ComparisonView onRevealTrash={vi.fn()} />);
  const dialog = view.getByRole("dialog");
  expect(opener.closest("[inert]")).not.toBeNull();
  expect(document.activeElement).toBe(view.getByRole("listbox"));
  const buttons = dialog.querySelectorAll<HTMLButtonElement>("button:not([disabled])");
  const last = buttons[buttons.length - 1];
  last.focus();
  fireEvent.keyDown(last, { key: "Tab" });
  expect(document.activeElement).toBe(buttons[0]);
  fireEvent.keyDown(buttons[0], { key: "Tab", shiftKey: true });
  expect(document.activeElement).toBe(last);
  act(() => useComparisonStore.setState({ open: false }));
  expect(opener.closest("[inert]")).toBeNull();
  expect(document.activeElement).toBe(opener);
  opener.remove();
});
