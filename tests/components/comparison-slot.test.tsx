// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import ComparisonSlot from "../../src/components/ComparisonSlot";

const MEMBER = {
  hash: "member-hash",
  fileName: "family.jpg",
  width: 4000,
  height: 3000,
  byteSize: 10,
  sharpness: 12,
  faceScore: null,
  copyCount: 1,
  hasThumb: true,
};

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("comparison pointer decisions", () => {
  it("single- and double-click both toggle exactly once", () => {
    const toggle = vi.fn();
    const view = render(
      <ComparisonSlot member={MEMBER} slotKey="1" kept={false} onToggle={toggle} />,
    );
    const slot = view.getByRole("button", { name: /Slot 1/ });

    fireEvent.click(slot, { detail: 1 });
    fireEvent.click(slot, { detail: 2 });
    fireEvent.doubleClick(slot, { detail: 2 });

    expect(toggle).toHaveBeenCalledOnce();
  });

  it("does not toggle when a hold inspects the original", () => {
    vi.useFakeTimers();
    const toggle = vi.fn();
    render(<ComparisonSlot member={MEMBER} slotKey="1" kept={false} onToggle={toggle} />);
    const image = screen.getByTitle("Press and hold for original pixels");

    fireEvent.pointerDown(image, { pointerId: 4, button: 0, isPrimary: true });
    act(() => vi.advanceTimersByTime(135));
    fireEvent.pointerUp(window, { pointerId: 4 });
    fireEvent.click(image, { detail: 1 });

    expect(toggle).not.toHaveBeenCalled();
  });
});
