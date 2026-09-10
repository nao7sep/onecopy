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
import ComparisonImageWindow from "../../src/windows/ComparisonImageWindow";
import type { ComparisonBroadcast } from "../../src/state/comparison-store";
import { useWindowPreferencesStore } from "../../src/state/window-preferences-store";
import {
  emitCalls,
  fireEvent,
  mockCommands,
  resetTauriMocks,
  close,
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
  useWindowPreferencesStore.setState({
    enlargeSmallImagesInPreview: true,
    videoTranscriptionEnabled: true,
    audioTranscriptionEnabled: true,
  });
});

afterEach(() => cleanup());

describe("the Comparison image window", () => {
  it("shows the whole picked image and routes Space/Escape only back to Comparison", async () => {
    const view = render(<ComparisonImageWindow />);
    await act(async () => {});
    await act(async () => fireEvent("comparison-image://state", {
      token: 7, sessionId: 1, returnWindow: "comparison-1",
      member: { hash: "image-hash", fileName: "picked.jpg", width: 4000, height: 3000 },
    }));
    expect(view.getByAltText("picked.jpg")).toBeTruthy();
    const press = (key: string, init: KeyboardEventInit = {}) => window.dispatchEvent(new KeyboardEvent("keydown", { key, cancelable: true, ...init }));
    press(" ", { repeat: true });
    press(" ", { isComposing: true });
    press("Escape", { ctrlKey: true });
    press("1");
    expect(emitCalls.some((call) => call.event.endsWith("://close"))).toBe(false);
    press(" ");
    press("Escape");
    expect(emitCalls.filter((call) => call.event === "comparison-image://close")).toEqual([
      { event: "comparison-image://close", payload: { token: 7 } },
      { event: "comparison-image://close", payload: { token: 7 } },
    ]);
    expect(emitCalls.some((call) => ["viewer://key", "comparison://key"].includes(call.event))).toBe(false);
  });
});

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

  it("loads its own managed-tool state and honors Main's projected transcription preference", async () => {
    useWindowPreferencesStore.setState({ videoTranscriptionEnabled: false });
    mockCommands({
      binaries_state: () => [
        {
          id: "ffmpeg", label: "ffmpeg", kind: "binary", status: "up-to-date",
          installedVersion: "9.0.1", facts: { latestKnownVersion: "9.0.1", lastCheckedAtUtc: null },
          path: "/tools/ffmpeg", requiredForCore: true, checkable: true,
          released: null, downloadBytes: null,
        },
        {
          id: "whisper-large-v3-turbo", label: "Whisper", kind: "model", status: "up-to-date",
          installedVersion: "model-pin", facts: { latestKnownVersion: "model-pin", lastCheckedAtUtc: null },
          path: "/models/whisper.bin", requiredForCore: false, checkable: false,
          released: "2024-10-01", downloadBytes: 1,
        },
      ],
      transcript_get: () => ({ status: "pending", text: null, message: null }),
    });
    const view = render(<PreviewWindow />);
    await act(async () => {});
    await act(async () => {
      fireEvent("preview://show", {
        hash: "manual-video",
        pathId: null,
        detail: { ...DETAIL, fileName: "manual.mp4", kind: "video", durationMs: 13_000 },
      });
    });
    await act(async () => {});
    view.getByRole("button", { name: "Expand" }).click();
    expect(view.getByRole("button", { name: "Transcribe this file" })).toBeTruthy();
  });

  it("forwards navigation, keeps Space inert, and requests live Preview fullscreen with F", async () => {
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
        repeat: false,
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
    expect(emitCalls).toContainEqual({ event: "preview://fullscreen", payload: "toggle" });
  });

  it("uses Escape to leave fullscreen before closing Preview and ignores repeated F", async () => {
    render(<PreviewWindow />);
    await act(async () => {});
    await act(async () => fireEvent("preview://fullscreen-state", { fullscreen: true, error: null }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "f", repeat: true }));
    expect(emitCalls.filter((call) => call.event === "preview://fullscreen"))
      .toEqual([{ event: "preview://fullscreen", payload: "exit" }]);
    expect(close).not.toHaveBeenCalled();
    await act(async () => fireEvent("preview://fullscreen-state", { fullscreen: false, error: null }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(close).toHaveBeenCalledOnce();
  });

  it("does not consume composing fullscreen or navigation keys", async () => {
    render(<PreviewWindow />);
    await act(async () => {});
    for (const key of ["f", "Escape", "ArrowRight", "Delete", " "]) {
      const event = new KeyboardEvent("keydown", { key, isComposing: true, cancelable: true });
      window.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
    }
    expect(emitCalls.some((call) => ["preview://fullscreen", "preview://key"].includes(call.event))).toBe(false);
  });
});

describe("a comparison window", () => {
  const broadcast: ComparisonBroadcast = {
    displayAspects: [16 / 9, 16 / 9],
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
          marked: false,
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
