// @vitest-environment happy-dom
import { cleanup, render } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import ShortcutsModal from "../../src/components/ShortcutsModal";
import { shortcutColumns } from "../../src/models/shortcuts";

afterEach(cleanup);

it("renders semantic columns in reading order with independently wrappable chords", () => {
  const view = render(<ShortcutsModal open onClose={() => {}} />);
  const columns = view.getByRole("dialog").querySelectorAll("[data-shortcut-column]");
  expect(columns).toHaveLength(3);
  for (const [index, column] of Array.from(columns).entries()) {
    expect(Array.from(column.querySelectorAll("section")).map((s) => s.getAttribute("aria-label")))
      .toEqual(shortcutColumns()[index].map((g) => g.title));
    for (const key of column.querySelectorAll("kbd")) {
      expect(key.className).toContain("[overflow-wrap:anywhere]");
      expect(key.parentElement?.className).toContain("max-w-[48%]");
    }
  }
  expect(view.getAllByRole("button", { name: "Close" }).some((button) => button.textContent === "Close")).toBe(true);
});
