// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import PassiveScrollRegion, {
  SCROLLBAR_HIDE_DELAY_MS,
  passiveScrollKey,
  scrollbarGeometry,
} from "../../src/components/ui/PassiveScrollRegion";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

function overflowingRegion() {
  const view = render(
    <PassiveScrollRegion label="History">
      <button type="button">Nested action</button>
    </PassiveScrollRegion>,
  );
  const viewport = screen.getByRole("region", { name: "History" }) as HTMLDivElement;
  Object.defineProperties(viewport, {
    clientHeight: { configurable: true, value: 200 },
    scrollHeight: { configurable: true, value: 1_000 },
    scrollTop: { configurable: true, writable: true, value: 0 },
  });
  fireEvent.scroll(viewport);
  const shell = viewport.parentElement as HTMLDivElement;
  return { ...view, shell, viewport };
}

describe("passive scroll decisions", () => {
  it("maps document keys without claiming unrelated keys", () => {
    expect(passiveScrollKey("ArrowDown", false, 200)).toBe(40);
    expect(passiveScrollKey("PageUp", false, 200)).toBe(-180);
    expect(passiveScrollKey(" ", true, 200)).toBe(-180);
    expect(passiveScrollKey("Home", false, 200)).toBe("start");
    expect(passiveScrollKey("Enter", false, 200)).toBeNull();
  });

  it("derives a bounded thumb from the viewport instead of keeping parallel state", () => {
    expect(scrollbarGeometry(0, 100, 100).overflow).toBe(false);
    const middle = scrollbarGeometry(400, 1_000, 200);
    expect(middle.overflow).toBe(true);
    expect(middle.thumbHeight).toBeCloseTo(38.4);
    expect(middle.thumbTop).toBeCloseTo(80.8);
    expect(scrollbarGeometry(800, 1_000, 200).thumbTop).toBeCloseTo(157.6);
  });
});

describe("PassiveScrollRegion", () => {
  it("owns scrolling keys but leaves nested controls alone", () => {
    const { viewport } = overflowingRegion();
    const scrollBy = vi.fn();
    Object.defineProperty(viewport, "scrollBy", { configurable: true, value: scrollBy });

    fireEvent.keyDown(viewport, { key: "PageDown" });
    expect(scrollBy).toHaveBeenCalledWith({ top: 180 });

    scrollBy.mockClear();
    fireEvent.keyDown(screen.getByRole("button", { name: "Nested action" }), { key: " " });
    expect(scrollBy).not.toHaveBeenCalled();
  });

  it("stays visible while focused and hides two seconds after focus leaves", () => {
    const { shell, viewport } = overflowingRegion();
    expect(shell.dataset.scrollbarVisible).toBe("true");

    fireEvent.focus(viewport);
    act(() => vi.advanceTimersByTime(SCROLLBAR_HIDE_DELAY_MS * 2));
    expect(shell.dataset.scrollbarVisible).toBe("true");

    fireEvent.blur(viewport);
    act(() => vi.advanceTimersByTime(SCROLLBAR_HIDE_DELAY_MS - 1));
    expect(shell.dataset.scrollbarVisible).toBe("true");
    act(() => vi.advanceTimersByTime(1));
    expect(shell.dataset.scrollbarVisible).toBe("false");
  });

  it("reveals near the edge and keeps dragging on the same scroll owner", () => {
    const { container, shell, viewport } = overflowingRegion();
    Object.defineProperty(shell, "getBoundingClientRect", {
      configurable: true,
      value: () => ({ left: 0, right: 200, top: 0, bottom: 200, width: 200, height: 200 }),
    });
    act(() => vi.advanceTimersByTime(SCROLLBAR_HIDE_DELAY_MS));
    expect(shell.dataset.scrollbarVisible).toBe("false");

    fireEvent.pointerMove(shell, { clientX: 190 });
    expect(shell.dataset.scrollbarVisible).toBe("true");

    const thumb = container.querySelector(".passive-scroll-thumb") as HTMLDivElement;
    Object.defineProperties(thumb, {
      setPointerCapture: { configurable: true, value: vi.fn() },
      hasPointerCapture: { configurable: true, value: () => true },
      releasePointerCapture: { configurable: true, value: vi.fn() },
    });
    fireEvent.pointerDown(thumb, { pointerId: 7, clientY: 20 });
    fireEvent.pointerMove(thumb, { pointerId: 7, clientY: 96.8 });
    expect(viewport.scrollTop).toBeCloseTo(400);
    fireEvent.pointerUp(thumb, { pointerId: 7, clientY: 96.8 });
  });
});
