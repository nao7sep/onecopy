// @vitest-environment happy-dom

// Text to be read scrolls inside a height limit, and macOS WebKit does not
// make a scrolling box focusable by itself: while the text overflows, the box
// is a named region the keyboard can reach and scroll; while it fits, it is
// plain text with no tab stop. The layout engine is not here, so these specs
// fake the one measurement the box reads.

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import BoundedText from "../../src/components/ui/BoundedText";
import OperationResult from "../../src/components/ui/OperationResult";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function contentHeight(scroll: number, client: number) {
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(scroll);
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(client);
}

describe("bounded text", () => {
  it("stays plain text with no tab stop while it fits", async () => {
    contentHeight(80, 80);
    const user = userEvent.setup();
    render(<BoundedText className="max-h-32">A short failure.</BoundedText>);
    const box = screen.getByText("A short failure.");
    expect(box.className.split(" ")).toEqual(expect.arrayContaining(["max-h-32", "overflow-y-auto"]));
    expect(box.hasAttribute("tabindex")).toBe(false);
    expect(box.hasAttribute("role")).toBe(false);
    expect(screen.queryByRole("region")).toBeNull();
    await user.tab();
    expect(document.activeElement).toBe(document.body);
  });

  it("becomes a focusable, named region while it overflows", async () => {
    contentHeight(400, 128);
    const user = userEvent.setup();
    render(<BoundedText className="max-h-32">ffmpeg exited with a long error</BoundedText>);
    const region = screen.getByRole("region", { name: "Full text" });
    expect(region.textContent).toBe("ffmpeg exited with a long error");
    expect(region.tabIndex).toBe(0);
    await user.tab();
    expect(document.activeElement).toBe(region);
  });

  it("bounds an operation result's text the same way", () => {
    contentHeight(400, 128);
    render(<OperationResult level="error">A tool's whole error output</OperationResult>);
    const region = screen.getByRole("region", { name: "Full text" });
    expect(region.className).toContain("max-h-32");
    expect(screen.getByRole("alert").contains(region)).toBe(true);
  });
});
