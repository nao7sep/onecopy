// @vitest-environment happy-dom

import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import RootErrorBoundary from "../../src/components/RootErrorBoundary";

vi.mock("../../src/repositories", () => ({
  log: { error: vi.fn() },
  toErrorFields: () => ({ error: { message: "render failed" } }),
}));

function Broken(): never {
  throw new Error("private detail");
}

describe("RootErrorBoundary", () => {
  it("leaves a private-detail-free reload surface after a render failure", () => {
    const onFailure = vi.fn();
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    render(
      <RootErrorBoundary onFailure={onFailure}>
        <Broken />
      </RootErrorBoundary>,
    );
    expect(
      screen.getByRole("heading", { name: "OneCopy needs to reload" }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Reload window" })).toBeTruthy();
    expect(screen.queryByText("private detail")).toBeNull();
    expect(onFailure).toHaveBeenCalledOnce();
    consoleError.mockRestore();
  });
});
