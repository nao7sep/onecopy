// @vitest-environment happy-dom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import ModalShell from "../../src/components/ModalShell";
import OperationResult from "../../src/components/ui/OperationResult";
import Button from "../../src/components/ui/Button";

afterEach(cleanup);

describe("modal result growth", () => {
  it("separates wrapping results from fixed actions and leaves scrolling to the body", () => {
    render(
      <ModalShell
        title="Settings"
        onClose={() => undefined}
        footerResult={
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
    expect(resultContainer?.className).toContain("break-words");
    expect(resultContainer?.className).not.toContain("overflow-y-auto");
    expect(resultContainer?.querySelector("[data-modal-actions]")).toBeNull();
    expect(screen.getByText("Close").closest("button")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
  });

  it("aligns footer metadata and action text by baseline and allows actions to wrap", () => {
    render(<ModalShell title="Activity" onClose={() => undefined}
      footerStart={<span>100 events loaded</span>} primaryAction={<Button>Another action</Button>}>
      Content
    </ModalShell>);
    const metadata = screen.getByText("100 events loaded").parentElement!;
    const actions = screen.getByText("Close", { selector: "button" }).parentElement!;
    expect(metadata.parentElement).toBe(actions.parentElement);
    expect(actions.parentElement?.className).toContain("items-baseline");
    expect(actions.className).toContain("items-baseline");
    expect(actions.className).toContain("flex-wrap");
    expect(metadata.className).toContain("min-w-0");
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
    expect(alert.className).toContain("items-start");
    expect(alert.querySelectorAll("svg")).toHaveLength(1);
    expect(dismiss.querySelector("svg")).not.toBeNull();
    expect(dismiss.className).toContain("-my-1");
  });

  it("centers a message beside its labelled action without changing top-aligned dismissals", () => {
    render(
      <OperationResult level="error" actions={<Button variant="ghost">Retry</Button>}>
        Playback controls could not be connected. Try again.
      </OperationResult>,
    );

    const alert = screen.getByRole("alert");
    expect(alert.className).toContain("items-center");
    expect(alert.className).not.toContain("items-start");
    expect(screen.getByRole("button", { name: "Retry" }).parentElement?.className).toContain("[&>button]:h-6");
  });
});
