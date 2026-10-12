// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { cleanup, render } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it } from "vitest";
import PreviewControl from "../../src/components/PreviewControl";
import { usePreviewStore } from "../../src/state/preview-store";

let previous: ReturnType<typeof usePreviewStore.getState>;
beforeEach(() => {
  previous = usePreviewStore.getState();
  usePreviewStore.setState({ follow: true, placementPreference: "split" });
});
afterEach(() => {
  cleanup();
  usePreviewStore.setState(previous);
});

it.each(["split", "window"] as const)("preserves the selected %s placement through hover and keyboard focus", async (placement) => {
  usePreviewStore.setState({ placementPreference: placement });
  const view = render(<PreviewControl />);
  const user = userEvent.setup();
  const buttons = view.getAllByRole("button");
  const selected = buttons[placement === "split" ? 1 : 2]!;
  const other = buttons[placement === "split" ? 2 : 1]!;
  expect(selected.getAttribute("aria-pressed")).toBe("true");
  expect(selected.className).toContain("bg-primary-surface");
  expect(selected.className).toContain("enabled:hover:brightness-95");
  expect(selected.className).not.toContain("outline-none");
  await user.hover(selected);
  selected.focus();
  await user.keyboard("{Shift}");
  expect(document.activeElement).toBe(selected);
  expect(selected.getAttribute("aria-pressed")).toBe("true");
  expect(other.getAttribute("aria-pressed")).toBe("false");
  expect(usePreviewStore.getState().placementPreference).toBe(placement);

  // These filled controls keep the app's separate button focus mark. This
  // pins the styling contract; native appearance remains a physical check.
  const css = readFileSync("src/App.css", "utf8");
  expect(css).toMatch(/@layer base \{\s*:is\(\s*button,[\s\S]*?\):focus-visible \{\s*outline: 2px solid var\(--focus-ring\);\s*outline-offset: 1px;/);
});
