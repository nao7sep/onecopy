// @vitest-environment happy-dom

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Menu, MenuItem } from "../../src/components/Menu";

afterEach(() => cleanup());

function commandMenu() {
  const commands = [vi.fn(), vi.fn(), vi.fn()];
  render(
    <Menu
      ariaLabel="Commands"
      trigger={(props) => <button {...props}>Open commands</button>}
    >
      <MenuItem disabled onSelect={() => undefined}>Unavailable</MenuItem>
      <MenuItem onSelect={commands[0]!}>First command</MenuItem>
      <MenuItem disabled onSelect={() => undefined}>Also unavailable</MenuItem>
      <MenuItem onSelect={commands[1]!}>Second command</MenuItem>
      <MenuItem onSelect={commands[2]!}>Last command</MenuItem>
    </Menu>,
  );
  return {
    commands,
    trigger: screen.getByRole("button", { name: "Open commands" }),
    item: (name: string) => screen.getByRole("menuitem", { name }),
  };
}

describe("independent pointer and keyboard menu navigation", () => {
  it("has its eligible target ready before the first opening event returns", () => {
    const menu = commandMenu();
    fireEvent.click(menu.trigger, { detail: 1 });
    expect(document.activeElement).toBe(menu.item("First command"));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    expect(document.activeElement).toBe(menu.item("Second command"));
  });

  it("initializes pointer opening quietly at the first eligible command, then moves on the first arrow", async () => {
    const user = userEvent.setup();
    const menu = commandMenu();
    await user.click(menu.trigger);
    expect(document.activeElement).toBe(menu.item("First command"));
    const panel = screen.getByRole("menu", { name: "Commands" });
    expect(panel.getAttribute("data-keyboard-navigation")).toBe("false");
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(menu.item("Second command"));
    expect(panel.getAttribute("data-keyboard-navigation")).toBe("true");
  });

  it.each(["{Enter}", " ", "{ArrowDown}"])("keyboard opening with %s immediately identifies the first eligible command", async (key) => {
    const user = userEvent.setup();
    const menu = commandMenu();
    menu.trigger.focus();
    await user.keyboard(key);
    expect(document.activeElement).toBe(menu.item("First command"));
    expect(screen.getByRole("menu").getAttribute("data-keyboard-navigation")).toBe("true");
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(menu.item("Second command"));
  });

  it("opens from ArrowUp at the last eligible command and moves upward from there", async () => {
    const user = userEvent.setup();
    const menu = commandMenu();
    menu.trigger.focus();
    await user.keyboard("{ArrowUp}");
    expect(document.activeElement).toBe(menu.item("Last command"));
    await user.keyboard("{ArrowUp}");
    expect(document.activeElement).toBe(menu.item("Second command"));
  });

  it("keeps the keyboard target while an earlier item is hovered and continues arrows and Enter from it", async () => {
    const user = userEvent.setup();
    const menu = commandMenu();
    await user.click(menu.trigger);
    await user.keyboard("{ArrowDown}{ArrowDown}");
    const last = menu.item("Last command");
    expect(document.activeElement).toBe(last);
    await user.hover(menu.item("First command"));
    fireEvent.pointerMove(menu.item("First command"));
    expect(document.activeElement).toBe(last);
    await user.keyboard("{ArrowUp}");
    expect(document.activeElement).toBe(menu.item("Second command"));
    await user.keyboard("{Enter}");
    expect(menu.commands[1]).toHaveBeenCalledOnce();
    expect(menu.commands[0]).not.toHaveBeenCalled();
    expect(menu.commands[2]).not.toHaveBeenCalled();
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(menu.trigger);
  });

  it("clicks the pointer's item even when a different command was the keyboard target", async () => {
    const user = userEvent.setup();
    const menu = commandMenu();
    await user.click(menu.trigger);
    await user.keyboard("{End}");
    expect(document.activeElement).toBe(menu.item("Last command"));
    await user.click(menu.item("First command"));
    expect(menu.commands[0]).toHaveBeenCalledOnce();
    expect(menu.commands[2]).not.toHaveBeenCalled();
  });

  it("type-ahead skips unavailable commands, shows the target and preserves it through pointer movement", async () => {
    const user = userEvent.setup();
    const menu = commandMenu();
    await user.click(menu.trigger);
    await user.keyboard("l");
    expect(document.activeElement).toBe(menu.item("Last command"));
    await user.hover(menu.item("First command"));
    await user.keyboard("{Enter}");
    expect(menu.commands[2]).toHaveBeenCalledOnce();
  });

  it("starts a later pointer opening quietly even after prior keyboard navigation", async () => {
    const user = userEvent.setup();
    const menu = commandMenu();
    await user.click(menu.trigger);
    await user.keyboard("{End}{Escape}");
    expect(document.activeElement).toBe(menu.trigger);
    await user.click(menu.trigger);
    expect(document.activeElement).toBe(menu.item("First command"));
    expect(screen.getByRole("menu").getAttribute("data-keyboard-navigation")).toBe("false");
  });
});

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

describe("keyboard fallthrough while a menu is open (R8-02)", () => {
  it("lets an unclaimed key (Backspace) reach the window's own command layer, but stops a claimed key", () => {
    render(
      <Menu
        ariaLabel="Test menu"
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
    const panel = screen.getByRole("menu", { name: "Test menu" });

    const seen: string[] = [];
    const onWindowKeyDown = (event: KeyboardEvent) => seen.push(event.key);
    window.addEventListener("keydown", onWindowKeyDown);
    try {
      fireEvent.keyDown(panel, { key: "Backspace", bubbles: true });
      expect(seen).toContain("Backspace");

      fireEvent.keyDown(panel, { key: "ArrowDown", bubbles: true });
      expect(seen).not.toContain("ArrowDown");
    } finally {
      window.removeEventListener("keydown", onWindowKeyDown);
    }
  });
});
