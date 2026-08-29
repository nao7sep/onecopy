// @vitest-environment happy-dom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import Wizard from "../../src/components/Wizard";
import { useWizardStore } from "../../src/state/wizard-store";

beforeEach(() => {
  useWizardStore.setState({
    open: true,
    step: 1,
    dirs: [],
    timezone: "UTC",
    timezoneValid: true,
    timezonePending: false,
    reconfigure: false,
    error: null,
  });
});

afterEach(() => cleanup());

describe("setup directories", () => {
  it("explains how to populate the required empty collection", () => {
    render(<Wizard />);

    expect(screen.getByText("Add at least one directory to continue.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add directory" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Next" }).hasAttribute("disabled")).toBe(true);
  });
});
