// @vitest-environment happy-dom

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { Menu, MenuItem } from "../../src/components/Menu";

afterEach(() => cleanup());

function rect({
  top,
  left,
  width,
  height,
}: {
  top: number;
  left: number;
  width: number;
  height: number;
}): DOMRect {
  return {
    top,
    left,
    right: left + width,
    bottom: top + height,
    width,
    height,
    x: left,
    y: top,
    toJSON: () => ({}),
  };
}

describe("the shared menu viewport boundary", () => {
  it("keeps a long portalled menu inside the viewport and repositions it", () => {
    let triggerRect = rect({ top: 350, left: 268, width: 26, height: 20 });
    let intrinsicHeight = 700;
    Object.defineProperties(window, {
      innerWidth: { configurable: true, value: 300, writable: true },
      innerHeight: { configurable: true, value: 400, writable: true },
    });
    const bounds = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: HTMLElement) {
        if (this.getAttribute("aria-label") === "Open test menu") return triggerRect;
        if (this.getAttribute("role") === "menu") {
          return rect({ top: 0, left: 0, width: 260, height: 700 });
        }
        return rect({ top: 0, left: 0, width: 0, height: 0 });
      });
    const scrollHeight = vi
      .spyOn(HTMLElement.prototype, "scrollHeight", "get")
      .mockImplementation(function (this: HTMLElement) {
        return this.getAttribute("role") === "menu" ? intrinsicHeight : 0;
      });

    try {
      render(
        <Menu
          ariaLabel="Test menu"
          align="end"
          trigger={(props) => (
            <button {...props} aria-label="Open test menu">
              Menu
            </button>
          )}
        >
          <MenuItem onSelect={() => undefined}>First command</MenuItem>
          <MenuItem onSelect={() => undefined}>Last command</MenuItem>
        </Menu>,
      );
      fireEvent.click(screen.getByRole("button", { name: "Open test menu" }));

      const menu = screen.getByRole("menu", { name: "Test menu" });
      expect(menu.parentElement).toBe(document.body);
      expect(menu.style.left).toBe("32px");
      expect(menu.style.top).toBe("8px");
      expect(menu.style.maxWidth).toBe("284px");
      expect(menu.style.maxHeight).toBe("384px");
      expect(menu.className).toContain("overflow-auto");

      triggerRect = rect({ top: 40, left: 430, width: 26, height: 20 });
      intrinsicHeight = 500;
      window.innerWidth = 500;
      window.innerHeight = 620;
      fireEvent(window, new Event("resize"));

      expect(menu.style.left).toBe("196px");
      expect(menu.style.top).toBe("64px");
      expect(menu.style.maxWidth).toBe("484px");
      expect(menu.style.maxHeight).toBe("548px");
    } finally {
      scrollHeight.mockRestore();
      bounds.mockRestore();
    }
  });
});
