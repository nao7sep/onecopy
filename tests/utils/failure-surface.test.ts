// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  presentEscapedFailure,
  recordInterfaceFailure,
} from "../../src/utils/failureSurface";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";
import { message } from "../../src/i18n/translate";

beforeEach(() => {
  resetTauriMocks();
  mockCommands({ record_interface_failure: () => null });
});

afterEach(() => {
  document.body.replaceChildren();
});

describe("escaped interface failures", () => {
  it("keeps one visible reload surface and updates its explanation", () => {
    presentEscapedFailure(message("crash.windowStopped"));
    presentEscapedFailure(message("crash.actionUnfinished"));

    expect(document.querySelectorAll("#onecopy-escaped-failure")).toHaveLength(1);
    expect(document.body.textContent).toContain("OneCopy needs to reload");
    expect(document.body.textContent).toContain("could not finish an action");
    expect(document.body.textContent).not.toContain("stopped unexpectedly");
  });

  it("asks the core to persist the current webview failure as its sentence's key", () => {
    recordInterfaceFailure(message("crash.drawingUnfinished"));

    expect(invokeCalls).toContainEqual({
      command: "record_interface_failure",
      args: { messageKey: "crash.drawingUnfinished", messageValues: null, detail: null },
    });
  });

  it("sends the real error as detail after the sentence", () => {
    recordInterfaceFailure(message("app.appearanceUpdateFailed"), new Error("x"));

    expect(invokeCalls).toContainEqual({
      command: "record_interface_failure",
      args: { messageKey: "app.appearanceUpdateFailed", messageValues: null, detail: "x" },
    });
  });

  it("shows the direct recovery surface when the core cannot save the failure", async () => {
    mockCommands({
      record_interface_failure: () => Promise.reject(new Error("index unavailable")),
    });

    recordInterfaceFailure(message("crash.drawingUnfinished"));
    await Promise.resolve();
    await Promise.resolve();

    expect(document.body.textContent).toContain("OneCopy needs to reload");
    expect(document.body.textContent).toContain("could not finish drawing");
    expect(document.body.textContent).toContain("could not save this failure");
  });
});
