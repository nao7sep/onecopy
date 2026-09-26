// @vitest-environment happy-dom
//
// src/main.tsx installs the last-resort window.onerror / unhandledrejection
// handlers exactly once, at module load, for every window this app opens.
// Nothing else in the tree exercises that wiring — everything downstream
// (presentEscapedFailure, recordInterfaceFailure) already has its own direct
// unit tests, so this file only proves the two global listeners are actually
// registered and call through to them, not merely defined.

import { describe, expect, it, vi } from "vitest";
import { invokeCalls, mockCommand, resetTauriMocks } from "./mocks/tauri";

describe("global escaped-failure wiring", () => {
  it("reports an uncaught error and an unhandled rejection through the same escaped-failure surface", async () => {
    resetTauriMocks({ keepListeners: true });
    mockCommand("record_interface_failure", () => null);

    // Importing the module runs its top-level side effects, including the
    // two addEventListener calls under test. There is no #root element and
    // media-use/appearance bootstrap has no other command mocked, so main.tsx's
    // own bootstrap chain fails and presents its own escaped failure first —
    // a side effect of import, not of this test. Wait for that unrelated
    // surface to settle before asserting on the handlers under test, so the
    // two do not race to write the same DOM node.
    await import("../src/main");
    await vi.waitFor(() =>
      expect(document.body.textContent).toContain("could not start safely"),
    );

    window.dispatchEvent(
      new ErrorEvent("error", { message: "boom", error: new Error("boom") }),
    );
    await vi.waitFor(() =>
      expect(document.body.textContent).toContain("stopped unexpectedly"),
    );
    expect(
      invokeCalls.filter((call) => call.command === "record_interface_failure")
        .some((call) => (call.args.message as string).includes("stopped unexpectedly")),
    ).toBe(true);

    const rejectionEvent = new Event("unhandledrejection") as PromiseRejectionEvent & Event;
    Object.assign(rejectionEvent, { reason: new Error("rejected"), promise: Promise.resolve() });
    window.dispatchEvent(rejectionEvent);
    await vi.waitFor(() =>
      expect(document.body.textContent).toContain("could not finish an action"),
    );
    expect(
      invokeCalls.filter((call) => call.command === "record_interface_failure")
        .some((call) => (call.args.message as string).includes("could not finish an action")),
    ).toBe(true);
  });
});
