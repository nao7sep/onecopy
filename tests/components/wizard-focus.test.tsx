// @vitest-environment happy-dom

import userEvent from "@testing-library/user-event";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
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
  it("keeps every page and long directory lists inside one body with a stable footer", () => {
    useWizardStore.setState({ dirs: Array.from({ length: 80 }, (_, index) => ({ path: `/photos/${index}` })) });
    render(<Wizard />);
    const surface = screen.getByRole("dialog");
    const [header, body, footer] = Array.from(surface.children) as HTMLElement[];
    const title = screen.getByRole("heading", { level: 1 });
    expect(surface.getAttribute("aria-labelledby")).toBe(title.id);
    expect(surface.getAttribute("aria-modal")).toBe("true");
    expect(surface.getAttribute("tabindex")).toBe("-1");
    expect(surface.classList.contains("max-h-[90vh]")).toBe(true);
    expect(header!.contains(title)).toBe(true);
    expect(header!.classList.contains("shrink-0")).toBe(true);
    expect(body!.classList.contains("min-h-0")).toBe(true);
    expect(body!.classList.contains("overflow-y-auto")).toBe(true);
    expect(footer!.classList.contains("shrink-0")).toBe(true);
    for (const step of [1, 2, 3, 4] as const) {
      act(() => useWizardStore.setState({ step, reconfigure: true }));
      expect(surface.querySelectorAll(".overflow-y-auto")).toHaveLength(1);
      expect(surface.querySelector("footer")).toBe(footer);
      expect(within(footer!).getByRole("button", { name: "Cancel" })).toBeTruthy();
      expect(within(footer!).getByRole("button", { name: step === 4 ? "Finish and scan" : "Next" })).toBeTruthy();
      expect(body!.querySelector("footer")).toBeNull();
      if (step === 2) expect(within(body!).getAllByRole("listitem")).toHaveLength(80);
    }
  });

  it("keeps page validation and finishing guards on the shared footer", () => {
    render(<Wizard />);
    act(() => useWizardStore.setState({ step: 2, dirs: [], reconfigure: true }));
    expect((screen.getByRole("button", { name: "Next" }) as HTMLButtonElement).disabled).toBe(true);
    act(() => useWizardStore.setState({ dirs: [{ path: "/photos" }] }));
    expect((screen.getByRole("button", { name: "Next" }) as HTMLButtonElement).disabled).toBe(false);
    act(() => useWizardStore.setState({ step: 3, timezone: " " }));
    expect((screen.getByRole("button", { name: "Next" }) as HTMLButtonElement).disabled).toBe(true);
    act(() => useWizardStore.setState({ timezone: "UTC" }));
    expect((screen.getByRole("button", { name: "Next" }) as HTMLButtonElement).disabled).toBe(false);
    act(() => useWizardStore.setState({ step: 4, finishing: true }));
    for (const name of ["Cancel", "Back", "Finishing…"]) {
      expect((screen.getByRole("button", { name }) as HTMLButtonElement).disabled).toBe(true);
    }
    for (const checkbox of screen.getAllByRole("checkbox")) {
      expect((checkbox as HTMLInputElement).disabled).toBe(true);
    }
  });

  it("cycles Tab inside each page, including after Next and Back replace its controls", async () => {
    const user = userEvent.setup();
    render(<Wizard />);
    for (const step of [1, 2, 3, 4] as const) {
      if (step > 1) next();
      const controls = screen.getByRole("dialog").querySelectorAll<HTMLElement>("button:not([disabled]), select, input");
      const first = controls[0]!;
      const last = controls[controls.length - 1]!;
      last.focus();
      await user.tab();
      expect(focused()).toBe(first);
      await user.tab({ shift: true });
      expect(focused()).toBe(last);
    }
    back();
    expect((focused() as HTMLSelectElement).value).toBe("UTC");
    await user.tab({ shift: true });
    expect(focused()).toBe(screen.getByRole("button", { name: "Next" }));
  });

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
