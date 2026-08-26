// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useEffect } from "react";
import { usePaneLayout } from "../../src/hooks/usePaneLayout";
import { useAppStore } from "../../src/state/app-store";

let resizeCallbacks: ResizeObserverCallback[] = [];
const originalPatchState = useAppStore.getState().patchState;

class ResizeObserverDouble {
  constructor(callback: ResizeObserverCallback) {
    resizeCallbacks.push(callback);
  }
  observe() {}
  unobserve() {}
  disconnect() {}
}

function Harness({ restoredState }: { restoredState?: Record<string, unknown> }) {
  const { contentRowRef, paneWidths, beginPaneDrag, restorePaneIntents } =
    usePaneLayout(true);

  useEffect(() => {
    if (restoredState !== undefined) restorePaneIntents(restoredState);
  }, [restorePaneIntents, restoredState]);

  return (
    <div ref={contentRowRef} data-testid="row">
      <output data-testid="widths">{JSON.stringify(paneWidths)}</output>
      <button onMouseDown={beginPaneDrag("left")}>Left divider</button>
      <button onMouseDown={beginPaneDrag("preview")}>Preview divider</button>
      <button onMouseDown={beginPaneDrag("right")}>Right divider</button>
    </div>
  );
}

function resizeRow(width: number) {
  const row = screen.getByTestId("row");
  Object.defineProperty(row, "clientWidth", { configurable: true, value: width });
  act(() => {
    for (const callback of resizeCallbacks) callback([], {} as ResizeObserver);
  });
}

beforeEach(() => {
  resizeCallbacks = [];
  vi.stubGlobal("ResizeObserver", ResizeObserverDouble);
});

afterEach(() => {
  cleanup();
  useAppStore.setState({ patchState: originalPatchState });
  vi.unstubAllGlobals();
});

describe("usePaneLayout", () => {
  it("derives resize clamps without persisting them", () => {
    const patchState = vi.fn<(patch: Record<string, unknown>) => Promise<void>>(
      async () => undefined,
    );
    useAppStore.setState({ patchState });
    render(
      <Harness
        restoredState={{ sidebarWidth: 600, rightPaneWidth: 400, previewPaneRatio: 0.7 }}
      />,
    );

    resizeRow(1107);

    expect(JSON.parse(screen.getByTestId("widths").textContent ?? "{}")).toEqual({
      left: 180,
      right: 220,
      preview: 260,
    });
    expect(patchState).not.toHaveBeenCalled();
  });

  it("persists only the center ratio when its divider drag ends", () => {
    const patchState = vi.fn<(patch: Record<string, unknown>) => Promise<void>>(
      async () => undefined,
    );
    useAppStore.setState({ patchState });
    render(<Harness />);
    resizeRow(2000);

    fireEvent.mouseDown(screen.getByRole("button", { name: "Preview divider" }), {
      clientX: 1000,
    });
    fireEvent.mouseMove(document, { clientX: 1100 });
    fireEvent.mouseUp(document, { clientX: 1100 });

    expect(patchState).toHaveBeenCalledOnce();
    expect(patchState).toHaveBeenCalledWith({ previewPaneRatio: expect.any(Number) });
    const ratio = patchState.mock.calls[0][0].previewPaneRatio as number;
    expect(ratio).toBeGreaterThan(0);
    expect(ratio).toBeLessThan(0.5);
  });

  it("persists only the fixed pane changed by a utility divider", () => {
    const patchState = vi.fn<(patch: Record<string, unknown>) => Promise<void>>(
      async () => undefined,
    );
    useAppStore.setState({ patchState });
    render(<Harness />);
    resizeRow(2000);

    fireEvent.mouseDown(screen.getByRole("button", { name: "Left divider" }), {
      clientX: 250,
    });
    // A real native drag can deliver its last movement and release before
    // React commits another render. Mouse-up itself is the authoritative final
    // coordinate; persistence must not sample a render ref.
    fireEvent.mouseUp(document, { clientX: 300 });

    expect(patchState).toHaveBeenCalledWith({ sidebarWidth: 306 });
  });
});
