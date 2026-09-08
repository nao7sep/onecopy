// @vitest-environment happy-dom

import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import MutationResultActions from "../../src/components/MutationResultActions";
import type { MutationResult } from "../../src/models/mutation";

function result(trashAvailable: boolean): MutationResult {
  return {
    operationId: 1,
    kind: "delete",
    cancelled: false,
    summary: {
      itemsCompleted: 2,
      itemsPartial: 0,
      itemsUnstarted: 0,
      filesCompleted: 2,
      filesFailed: 0,
      filesUnstarted: 0,
      trashAvailable,
      error: null,
    },
  };
}

afterEach(cleanup);

describe("persistent mutation result remedies", () => {
  it("offers Deleted files only when the backend reports recovery material", () => {
    const reveal = vi.fn();
    const dismiss = vi.fn();
    const view = render(
      <MutationResultActions
        result={result(true)}
        onRevealTrash={reveal}
        onDismiss={dismiss}
      />,
    );

    fireEvent.click(
      view.getByRole("button", { name: "Reveal deleted files…" }),
    );
    expect(reveal).toHaveBeenCalledOnce();
    expect(
      view.getByRole("button", { name: "Dismiss file-operation result" }),
    ).toBeTruthy();

    view.rerender(
      <MutationResultActions
        result={result(false)}
        onRevealTrash={reveal}
        onDismiss={dismiss}
      />,
    );
    expect(
      view.queryByRole("button", { name: "Reveal deleted files…" }),
    ).toBeNull();
  });
});
