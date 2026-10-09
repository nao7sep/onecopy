// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import Wizard from "../../src/components/Wizard";
import { useWizardStore } from "../../src/state/wizard-store";

beforeEach(() => {
  useWizardStore.setState({
    open: true,
    step: 1,
    dirs: [{ path: "/photos" }],
    language: "en",
    timezone: "UTC",
    reconfigure: false,
    error: null,
    finishing: false,
  });
});

afterEach(() => cleanup());

const next = () => fireEvent.click(screen.getByRole("button", { name: "Next" }));
const back = () => fireEvent.click(screen.getByRole("button", { name: "Back" }));
const focused = () => document.activeElement as HTMLElement;

describe("setup pages", () => {
  it("asks one thing per page and focuses each page's first control, by Next and by Back", () => {
    render(<Wizard />);

    expect(screen.getByText("Step 1 of 4")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Back" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
    expect(focused().tagName).toBe("SELECT");
    expect((focused() as HTMLSelectElement).value).toBe("en");

    next();
    expect(screen.getByText("Step 2 of 4")).toBeTruthy();
    expect(focused()).toBe(screen.getByRole("button", { name: "Add directory" }));

    next();
    expect(screen.getByText("Step 3 of 4")).toBeTruthy();
    expect(focused().tagName).toBe("SELECT");
    expect((focused() as HTMLSelectElement).value).toBe("UTC");

    next();
    expect(screen.getByText("Step 4 of 4")).toBeTruthy();
    expect(focused()).toBe(screen.getAllByRole("checkbox")[0]);

    back();
    expect((focused() as HTMLSelectElement).value).toBe("UTC");
    back();
    expect(focused()).toBe(screen.getByRole("button", { name: "Add directory" }));
    back();
    expect((focused() as HTMLSelectElement).value).toBe("en");
  });
});
