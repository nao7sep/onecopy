// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import DestinationConflictModal from "../../src/components/DestinationConflictModal";

afterEach(cleanup);

it("starts conflict review safely on Cancel and navigates the available footer choices", () => {
  render(<DestinationConflictModal pending={{ destDir: "/fixture", mode: "copy", planToken: "plan",
    selection: { items: [{ hash: "photo", pathId: null }], anchorKey: "photo" },
    conflicts: [{ path: "/fixture/photo.jpg", incomingBytes: 10, existingBytes: 12,
      withinSelection: false, preservedPaths: ["/fixture/photo.jpg"] }], overwriteAllowed: false }} />);
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Cancel" }));
  fireEvent.keyDown(document.activeElement!, { key: "ArrowRight" });
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Rename and copy" }));
  fireEvent.keyDown(document.activeElement!, { key: "ArrowRight" });
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Rename and copy" }));
});
