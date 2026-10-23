// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import RevealCopiesModal from "../../src/components/RevealCopiesModal";
import {
  mockCommands,
  resetTauriMocks,
  invokeCalls,
} from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    reveal_native_path: () => null,
    get_item_detail: () => ({
      fileName: "photo.jpg",
      kind: "image",
      byteSize: 100,
      width: 10,
      height: 10,
      durationMs: null,
      dateState: "dated",
      resolvedUtcMs: 1,
      resolvedSource: "filesystem",
      dateOnly: false,
      copyPaths: ["/one/photo.jpg", "/two/photo.jpg"], copyCount: 2,
      companionPaths: [],
      stripFrames: null,
    }),
  });
});

afterEach(cleanup);

describe("Comparison physical-copy reveal", () => {
  it("loads every copy and lets the user choose one", async () => {
    const view = render(
      <RevealCopiesModal
        hash="hash-a"
        fileName="photo.jpg"
        onClose={() => undefined}
      />,
    );
    await act(async () => {});

    fireEvent.click(view.getByRole("button", { name: "/two/photo.jpg" }));
    expect(invokeCalls.find((call) => call.command === "reveal_native_path")?.args).toEqual({ path: "/two/photo.jpg" });
  });
});


it("shows distinct escaped native names and reveals their unchanged identity", async () => {
  const paths = ["/one/photo\u0000bfe.jpg", "/one/photo\u0000bff.jpg", "/one/photo\ufffd.jpg"];
  mockCommands({ get_item_detail: () => ({ copyPaths: paths }) });
  const view = render(<RevealCopiesModal hash="hash-native" fileName={"photo\u0000bff.jpg"} onClose={() => undefined} />);
  await act(async () => {});
  expect(view.getByRole("dialog", { name: "Show a copy of photo\\xFF.jpg" })).toBeTruthy();
  expect(view.getByRole("button", { name: "/one/photo\\xFE.jpg" })).toBeTruthy();
  expect(view.getByRole("button", { name: "/one/photo\ufffd.jpg" })).toBeTruthy();
  fireEvent.click(view.getByRole("button", { name: "/one/photo\\xFF.jpg" }));
  expect(invokeCalls.find((call) => call.command === "reveal_native_path")?.args).toEqual({ path: paths[1] });
});
