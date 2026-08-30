// @vitest-environment happy-dom

import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import ComparisonSlot from "../../src/components/ComparisonSlot";

const MEMBER = {
  hash: "hash-a",
  fileName: "photo.jpg",
  width: 4000,
  height: 3000,
  byteSize: 1000,
  sharpness: 14,
  faceScore: null,
  copyCount: 2,
  hasThumb: true,
};

afterEach(cleanup);

function renderSlot() {
  const select = vi.fn();
  const decide = vi.fn();
  const reveal = vi.fn();
  const view = render(
    <ComparisonSlot
      member={MEMBER}
      slotKey="0"
      selected={false}
      anchor={false}
      onSelect={select}
      onDecide={decide}
      onReveal={reveal}
    />,
  );
  return { ...view, select, decide, reveal };
}

describe("comparison pointer selection", () => {
  it("uses ordinary, additive, and range selection like Main", () => {
    const { getByRole, select } = renderSlot();
    const card = getByRole("option");
    fireEvent.click(card);
    fireEvent.click(card, { metaKey: true });
    fireEvent.click(card, { shiftKey: true });
    expect(select.mock.calls.map(([mode]) => mode)).toEqual([
      "exclusive",
      "toggle",
      "range",
    ]);
  });

  it("double-click exclusively selects then decides the visible page", () => {
    const { getByRole, select, decide } = renderSlot();
    fireEvent.doubleClick(getByRole("option"));
    expect(select).toHaveBeenLastCalledWith("exclusive");
    expect(decide).toHaveBeenCalledOnce();
  });

  it("prints the assigned direct key and selection semantics", () => {
    const { getByRole, getByText } = renderSlot();
    expect(getByRole("option").getAttribute("aria-selected")).toBe("false");
    expect(getByText("0")).toBeTruthy();
  });

  it("selects the card before either physical-file action", () => {
    const { getByRole, select, reveal } = renderSlot();
    fireEvent.click(
      getByRole("button", { name: "Choose a copy of photo.jpg to reveal" }),
    );
    expect(select).toHaveBeenLastCalledWith("exclusive");
    expect(reveal).toHaveBeenCalledOnce();
  });

  it("keeps the card and its actions when the preview fails", () => {
    const { getByRole, getByText } = renderSlot();
    fireEvent.error(getByRole("img", { name: "photo.jpg" }));
    expect(getByText(/Preview unavailable/)).toBeTruthy();
    expect(
      getByRole("button", { name: "Open photo.jpg in default app" }),
    ).toBeTruthy();
  });
});
