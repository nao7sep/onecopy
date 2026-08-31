// @vitest-environment happy-dom

import { cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { useDestinationDragBoundary } from "../../src/hooks/useDestinationDragBoundary";
import { useDestinationsStore } from "../../src/state/destinations-store";

function Harness() {
  useDestinationDragBoundary();
  return <input aria-label="Editor" />;
}

function dragEvent(type: "dragover" | "drop", types: string[]): DragEvent {
  const event = new Event(type, { bubbles: true, cancelable: true }) as DragEvent;
  Object.defineProperty(event, "dataTransfer", {
    value: { types, dropEffect: "copy" },
  });
  return event;
}

beforeEach(() => {
  useDestinationsStore.setState({
    dragSelection: null,
  });
});

afterEach(() => {
  cleanup();
});

describe("the app-wide drag boundary", () => {
  it("denies an external file even when the Destinations tab is absent", () => {
    const view = render(<Harness />);
    const event = dragEvent("dragover", ["Files"]);

    view.container.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(true);
    expect(event.dataTransfer?.dropEffect).toBe("none");
  });

  it("keeps ordinary text drops native in a real editor", () => {
    const view = render(<Harness />);
    const input = view.getByLabelText("Editor");
    const over = dragEvent("dragover", ["text/plain"]);
    const drop = dragEvent("drop", ["text/plain"]);

    input.dispatchEvent(over);
    input.dispatchEvent(drop);

    expect(over.defaultPrevented).toBe(false);
    expect(drop.defaultPrevented).toBe(false);
  });

  it("never turns OneCopy's internal text transport into editor content", () => {
    const view = render(<Harness />);
    useDestinationsStore.setState({
      dragSelection: {
        items: [{ hash: "h1", pathId: null }],
        anchorKey: "h1",
      },
    });
    const event = dragEvent("dragover", ["text/plain"]);

    view.getByLabelText("Editor").dispatchEvent(event);

    expect(event.defaultPrevented).toBe(true);
  });

});
