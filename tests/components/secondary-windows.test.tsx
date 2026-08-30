// @vitest-environment happy-dom
//
// The two secondary windows, mounted for real. The developer reports both
// blank ("white; nothing is displayed") — a render-time throw in either
// component would produce exactly that, since React tears the tree down to an
// empty root. These specs mount each window, drive its handshake end to end,
// and assert the content actually appears, so the failure — wherever it is —
// is at least narrowed to the real-webview layer (module evaluation, CSP,
// permissions) rather than the components.

import { beforeEach, afterEach, describe, expect, it } from "vitest";
import { render, cleanup, act } from "@testing-library/react";
import PreviewWindow from "../../src/windows/PreviewWindow";
import ComparisonWindow from "../../src/windows/ComparisonWindow";
import type { ComparisonBroadcast } from "../../src/state/comparison-store";
import {
  emitCalls,
  fireEvent,
  mockCommands,
  resetTauriMocks,
} from "../mocks/tauri";

const DETAIL = {
  fileName: "IMG_1.jpg",
  kind: "image",
  byteSize: 1000,
  width: 4000,
  height: 3000,
  durationMs: null,
  dateState: "undated" as const,
  resolvedUtcMs: null,
  resolvedSource: null,
  dateOnly: false,
  copyPaths: ["/vol/IMG_1.jpg"],
  companionPaths: [],
  stripFrames: null,
};

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({ logging_debug_enabled: () => false, log_event: () => null });
});

afterEach(() => cleanup());

describe("the preview window", () => {
  it("mounts without throwing and shows its placeholder", () => {
    const view = render(<PreviewWindow />);
    expect(view.container.textContent).toContain(
      "Select an item in the main window",
    );
  });

  it("announces itself only AFTER its listener is registered", async () => {
    render(<PreviewWindow />);
    // Let the async listen settle.
    await act(async () => {});
    const ready = emitCalls.find((c) => c.event === "preview://ready");
    expect(ready).toBeDefined();
  });

  it("renders the image when the show message arrives", async () => {
    const view = render(<PreviewWindow />);
    await act(async () => {});
    await act(async () => {
      fireEvent("preview://show", {
        hash: "abc",
        pathId: null,
        detail: DETAIL,
      });
    });
    const img = view.container.querySelector("img");
    expect(img).not.toBeNull();
    expect(img?.getAttribute("src")).toContain("preview-abc");
  });

  it("forwards navigation, keeps Space inert, and requests shared fullscreen with F", async () => {
    render(<PreviewWindow />);
    await act(async () => {});

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight" }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: " " }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "f" }));
    await act(async () => {});

    expect(emitCalls).toContainEqual({
      event: "preview://key",
      payload: {
        key: "ArrowRight",
        code: "",
        shiftKey: false,
        metaKey: false,
        ctrlKey: false,
        altKey: false,
      },
    });
    expect(
      emitCalls.some(
        (call) =>
          call.event === "preview://key" &&
          (call.payload as { key?: string }).key === " ",
      ),
    ).toBe(false);
    expect(
      emitCalls.some((call) => call.event === "preview://fullscreen"),
    ).toBe(true);
  });
});

describe("a comparison window", () => {
  const broadcast: ComparisonBroadcast = {
    chunks: [
      [],
      [
        {
          member: {
            hash: "m1",
            fileName: "a.jpg",
            width: 4000,
            height: 3000,
            byteSize: 5_000_000,
            sharpness: 12,
            faceScore: null,
            copyCount: 2,
            hasThumb: true,
          },
          slotKey: "0",
          selected: false,
          anchor: false,
        },
      ],
    ],
    page: 0,
    pageCount: 2,
    remainingCount: 5,
    portraitDominant: false,
  };

  it("mounts without throwing and waits", () => {
    const view = render(<ComparisonWindow slice={1} />);
    expect(view.container.textContent).toContain("Waiting for the comparison…");
  });

  it("renders its slice when the state broadcast arrives", async () => {
    const view = render(<ComparisonWindow slice={1} />);
    await act(async () => {});
    await act(async () => {
      fireEvent("comparison://state", broadcast);
    });
    expect(view.container.textContent).toContain("a.jpg");
    expect(view.container.textContent).toContain("0");
    // The facts that make the keep decision possible.
    expect(view.container.textContent).toContain("4000×3000");
  });

  it("forwards only assigned comparison commands", async () => {
    render(<ComparisonWindow slice={1} />);
    await act(async () => {});
    await act(async () => {
      fireEvent("comparison://state", broadcast);
    });

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "0" }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "f" }));
    window.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowRight", metaKey: true }),
    );
    await act(async () => {});

    expect(
      emitCalls.filter((call) => call.event === "comparison://key"),
    ).toHaveLength(1);
  });

  it("announced readiness after listening, so the reply can be heard", async () => {
    render(<ComparisonWindow slice={1} />);
    await act(async () => {});
    expect(emitCalls.some((c) => c.event === "comparison://ready")).toBe(true);
  });
});
