// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import RevealCopiesDialog from "../../src/components/RevealCopiesDialog";
import {
  mockCommands,
  resetTauriMocks,
  revealItemInDir,
} from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
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
      copyPaths: ["/one/photo.jpg", "/two/photo.jpg"],
      companionPaths: [],
      stripFrames: null,
    }),
  });
});

afterEach(cleanup);

describe("Comparison physical-copy reveal", () => {
  it("loads every copy and lets the user choose one", async () => {
    const view = render(
      <RevealCopiesDialog
        hash="hash-a"
        fileName="photo.jpg"
        onClose={() => undefined}
      />,
    );
    await act(async () => {});

    fireEvent.click(view.getByRole("button", { name: "/two/photo.jpg" }));
    expect(revealItemInDir).toHaveBeenCalledWith("/two/photo.jpg");
  });
});
