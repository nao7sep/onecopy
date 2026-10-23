// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ComparisonSlot from "../../src/components/ComparisonSlot";
import type { GroupMember } from "../../src/state/comparison-store";
import { seedAppConfig } from "../helpers/config";
import { useWindowPreferencesStore } from "../../src/state/window-preferences-store";
import { useAppStore } from "../../src/state/app-store";

const MEMBER: GroupMember = {
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

beforeEach(() => { seedAppConfig(); useWindowPreferencesStore.setState({ scoreFaces: true }); });

afterEach(cleanup);

function renderSlot(member: GroupMember = MEMBER) {
  const select = vi.fn();
  const reveal = vi.fn();
  const view = render(
    <ComparisonSlot
      member={member}
      slotKey="0"
      marked={false}
      anchor={false}
      onSelect={select}
      onReveal={reveal}
    />,
  );
  return { ...view, select, reveal };
}

describe("comparison pointer decisions", () => {
  it("separates ordinary activation from explicit toggle and range gestures", () => {
    const { getByRole, select } = renderSlot();
    const card = getByRole("option");
    fireEvent.click(card);
    fireEvent.click(card, { metaKey: true });
    fireEvent.click(card, { shiftKey: true });
    expect(select.mock.calls.map(([mode]) => mode)).toEqual([
      "activate",
      "toggle",
      "range",
    ]);
  });

  it("double-click only activates the card", () => {
    const { getByRole, select } = renderSlot();
    fireEvent.doubleClick(getByRole("option"));
    expect(select).toHaveBeenLastCalledWith("activate");
  });

  it("offers a visible keep-mark toggle", () => {
    const { getByRole, select } = renderSlot();
    fireEvent.click(getByRole("button", { name: "Keep photo.jpg" }));
    expect(select).toHaveBeenLastCalledWith("toggle");
  });

  it("prints the assigned direct key and selection semantics", () => {
    const { getByRole, getByText } = renderSlot();
    expect(getByRole("option").getAttribute("aria-selected")).toBe("false");
    expect(getByText("0")).toBeTruthy();
  });

  it("shows existing face scores only while face scoring is enabled", () => {
    useAppStore.setState({ appData: null });
    useWindowPreferencesStore.setState({ scoreFaces: false });
    const view = renderSlot({ ...MEMBER, faceScore: 0.66 });
    expect(view.queryByRole("img", { name: /Advisory:/ })).toBeNull();
    act(() => useWindowPreferencesStore.setState({ scoreFaces: true }));
    expect(view.getByRole("img", { name: /Advisory:/ })).toBeTruthy();
    act(() => useWindowPreferencesStore.setState({ scoreFaces: false }));
    expect(view.queryByRole("img", { name: /Advisory:/ })).toBeNull();
  });

  it("draws face ratings as icons rather than font characters", () => {
    const { getByRole } = renderSlot({ ...MEMBER, faceScore: 0.66 });
    const rating = getByRole("img", {
      name: "Advisory: 3 face stars — best-face confidence and smile hint",
    });

    expect(rating.querySelectorAll("svg.lucide-star")).toHaveLength(3);
    expect(rating.textContent).not.toContain("★");
  });

  it("selects the card before either physical-file action", () => {
    const { getByRole, select, reveal } = renderSlot();
    fireEvent.click(
      getByRole("button", { name: "Choose a copy of photo.jpg to reveal" }),
    );
    expect(select).toHaveBeenLastCalledWith("activate");
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
