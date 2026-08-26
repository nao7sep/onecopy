// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useHoldInspect, type PointerPoint } from "../../src/hooks/useHoldInspect";

function Harness({
  onStart,
  onMove,
  onEnd,
}: {
  onStart: (point: PointerPoint) => void;
  onMove: (point: PointerPoint) => void;
  onEnd: () => void;
}) {
  const hold = useHoldInspect({ onStart, onMove, onEnd });
  return (
    <div
      data-testid="target"
      data-inspecting={hold.inspecting}
      onPointerDown={hold.onPointerDown}
      onClickCapture={hold.onClickCapture}
    />
  );
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("useHoldInspect", () => {
  it("publishes captured movement after the threshold", () => {
    const start = vi.fn();
    const move = vi.fn();
    render(<Harness onStart={start} onMove={move} onEnd={() => undefined} />);
    const target = screen.getByTestId("target");

    fireEvent.pointerDown(target, {
      pointerId: 1,
      button: 0,
      isPrimary: true,
      clientX: 10,
      clientY: 20,
    });
    act(() => vi.advanceTimersByTime(135));
    fireEvent.pointerMove(window, { pointerId: 1, clientX: 30, clientY: 40 });

    expect(start).toHaveBeenCalledWith({ clientX: 10, clientY: 20 });
    expect(move).toHaveBeenCalledWith({ clientX: 30, clientY: 40 });
  });

  it.each([
    ["pointer cancellation", () => fireEvent.pointerCancel(window, { pointerId: 2 })],
    ["lost capture", () => fireEvent.lostPointerCapture(window, { pointerId: 2 })],
    ["window blur", () => fireEvent.blur(window)],
  ])("ends on %s", (_name, cancel) => {
    const end = vi.fn();
    render(<Harness onStart={() => undefined} onMove={() => undefined} onEnd={end} />);
    const target = screen.getByTestId("target");
    fireEvent.pointerDown(target, { pointerId: 2, button: 0, isPrimary: true });
    act(() => vi.advanceTimersByTime(135));

    cancel();

    expect(end).toHaveBeenCalledOnce();
    expect(target.dataset.inspecting).toBe("false");
  });

  it("ends active inspection on unmount", () => {
    const end = vi.fn();
    const view = render(
      <Harness onStart={() => undefined} onMove={() => undefined} onEnd={end} />,
    );
    fireEvent.pointerDown(screen.getByTestId("target"), {
      pointerId: 3,
      button: 0,
      isPrimary: true,
    });
    act(() => vi.advanceTimersByTime(135));

    view.unmount();

    expect(end).toHaveBeenCalledOnce();
  });
});
