// @vitest-environment happy-dom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import ModalShell from "../../src/components/ModalShell";
import OperationResult from "../../src/components/ui/OperationResult";
import Button from "../../src/components/ui/Button";

afterEach(cleanup);

describe("modal result growth", () => {
  it("bounds the result and body while keeping footer actions fixed and reachable", () => {
    render(
      <ModalShell
        title="Settings"
        onClose={() => undefined}
        footerStart={
          <OperationResult level="error">
            A detailed failure that may wrap across several lines without
            displacing the controls that let the user leave or retry.
          </OperationResult>
        }
        primaryAction={<Button>Retry</Button>}
      >
        <label>
          Visible setting
          <input />
        </label>
      </ModalShell>,
    );

    const dialog = screen.getByRole("dialog");
    const resultContainer = screen.getByRole("alert").parentElement;
    const body = screen.getByText("Visible setting").parentElement;

    expect(dialog.className).toContain("max-h-[90vh]");
    expect(dialog.className).toContain("flex-col");
    expect(body?.className).toContain("min-h-0");
    expect(body?.className).toContain("flex-1");
    expect(body?.className).toContain("overflow-y-auto");
    expect(resultContainer?.className).toContain("max-h-24");
    expect(resultContainer?.className).toContain("overflow-y-auto");
    expect(screen.getByText("Close").closest("button")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
  });

  it("renders only the quiet dismiss mark, with no severity decoration or prefix", () => {
    render(
      <OperationResult level="error" onDismiss={() => undefined}>
        The settings could not be saved.
      </OperationResult>,
    );

    const alert = screen.getByRole("alert");
    const dismiss = screen.getByRole("button", { name: "Dismiss result" });
    expect(alert.textContent).toBe("The settings could not be saved.");
    expect(alert.querySelectorAll("svg")).toHaveLength(1);
    expect(dismiss.querySelector("svg")).not.toBeNull();
  });
});
