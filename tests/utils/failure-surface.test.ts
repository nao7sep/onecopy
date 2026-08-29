// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  presentEscapedFailure,
  recordInterfaceFailure,
} from "../../src/utils/failureSurface";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks();
  mockCommands({ record_interface_failure: () => null });
});

afterEach(() => {
  document.body.replaceChildren();
});

describe("escaped interface failures", () => {
  it("keeps one visible reload surface and updates its explanation", () => {
    presentEscapedFailure("first failure");
    presentEscapedFailure("latest failure");

    expect(document.querySelectorAll("#onecopy-escaped-failure")).toHaveLength(1);
    expect(document.body.textContent).toContain("OneCopy needs to reload");
    expect(document.body.textContent).toContain("latest failure");
    expect(document.body.textContent).not.toContain("first failure");
  });

  it("asks the core to persist the current webview failure", () => {
    recordInterfaceFailure("drawing failed");

    expect(invokeCalls).toContainEqual({
      command: "record_interface_failure",
      args: { message: "drawing failed" },
    });
  });
});
