// @vitest-environment happy-dom

import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
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
});
