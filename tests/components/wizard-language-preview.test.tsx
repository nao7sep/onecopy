// @vitest-environment happy-dom

// R5.5 D-L3: the wizard previews the chosen language inside its own view and
// commits it only on Finish. Nothing outside the wizard's own rendered
// subtree — the document's declared language, or the shared language store
// every other window reads — may see the choice before that.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import Wizard from "../../src/components/Wizard";
import { useWizardStore } from "../../src/state/wizard-store";
import { useLanguageStore } from "../../src/state/language-store";

beforeEach(() => {
  document.documentElement.lang = "en";
  useLanguageStore.setState({ language: "en", systemLanguage: "en", systemLocale: null });
  useWizardStore.setState({
    open: true,
    step: 1,
    dirs: [],
    language: "system",
    timezone: "UTC",
    reconfigure: false,
    error: null,
  });
});

afterEach(() => cleanup());

describe("wizard language preview", () => {
  it("shows the wizard's own text in the chosen language without touching the document or the shared store", () => {
    render(<Wizard />);
    expect(screen.getByText("Setup")).toBeTruthy();

    fireEvent.change(screen.getByRole("combobox"), { target: { value: "ja" } });

    // The wizard's own subtree follows the choice at once...
    expect(screen.getByText("セットアップ")).toBeTruthy();
    // ...but nothing outside it does.
    expect(document.documentElement.lang).toBe("en");
    expect(useLanguageStore.getState().language).toBe("en");
  });

  it("leaves the shared language store untouched after Cancel on a re-run", () => {
    useWizardStore.setState({ reconfigure: true });
    render(<Wizard />);

    fireEvent.change(screen.getByRole("combobox"), { target: { value: "ja" } });
    const cancelButton = screen.getByText("キャンセル");
    expect(cancelButton).toBeTruthy();

    fireEvent.click(cancelButton);

    expect(useWizardStore.getState().open).toBe(false);
    expect(useLanguageStore.getState().language).toBe("en");
    expect(document.documentElement.lang).toBe("en");
  });

  // R5.5 C1: the wizard offers the SAME choice as Settings -- System plus
  // exactly the ten supported languages, each named in its own words.
  it("lists System plus the ten languages, each named in its own words", async () => {
    const { CATALOGUES } = await import("../../src/i18n/catalogues");
    const { LANGUAGES } = await import("../../src/i18n/languages");
    render(<Wizard />);

    const select = screen.getByRole("combobox") as HTMLSelectElement;
    const options = [...select.options];
    expect(options).toHaveLength(LANGUAGES.length + 1);
    expect(options[0].value).toBe("system");
    expect(options[0].textContent).toBe("System");
    for (const language of LANGUAGES) {
      const option = options.find((candidate) => candidate.value === language)!;
      expect(option).toBeTruthy();
      expect(option.textContent).toBe(CATALOGUES[language]["language.name"]);
      expect(option.getAttribute("lang")).toBe(language);
    }
  });
});
