// @vitest-environment happy-dom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { cleanup, render } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import ShortcutsModal from "../../src/components/ShortcutsModal";
import { shortcutColumns } from "../../src/models/shortcuts";
import { t } from "../helpers/i18n";

afterEach(cleanup);

const REM = 16;
const read = (path: string) => readFileSync(resolve(path), "utf8");
const number = (source: string, pattern: RegExp, what: string) => {
  const match = pattern.exec(source);
  expect(match, `${what} not found`).not.toBeNull();
  return Number(match![1]);
};

// Three columns fitting is arithmetic across three files that nothing renders in
// this environment: the window's opening width, the two width bounds the shell
// puts on the surface, and the grid's own track minimum. They disagreed once —
// the surface came out eight pixels short of three 19rem tracks, so the third
// column wrapped under the first two and the modal stood twice as tall at the
// only width most people ever see it. Derive it rather than eyeballing it.
it("fits its three columns in the surface the window's opening width allows", () => {
  const modal = read("src/components/ShortcutsModal.tsx");
  const shell = read("src/components/ModalShell.tsx");
  const conf = JSON.parse(read("src-tauri/tauri.conf.json")) as { app: { windows: { width: number }[] } };

  const windowWidth = conf.app.windows[0].width;
  const surface = Math.min(
    number(modal, /widthClass="w-\[min\((\d+)px,/, "the surface's fixed cap"),
    windowWidth - number(modal, /calc\(100vw-(\d+)rem\)/, "the surface's viewport margin") * REM,
    (windowWidth * number(shell, /max-w-\[(\d+)vw\]/, "the shell's viewport cap")) / 100,
  );
  // The body is the scroller and App.css gives every bar a definite 16px, so the
  // gutter is content width this grid never gets.
  const content =
    surface - number(shell, /overflow-y-auto px-(\d+)/, "the body's padding") * 4 * 2 - 16;

  const track = number(modal, /minmax\(min\(100%,([\d.]+)rem\)/, "the track minimum") * REM;
  const gap = number(modal, /\bgap-(\d+)\b/, "the column gap") * 4;
  expect(3 * track + 2 * gap).toBeLessThanOrEqual(content);
});

it("renders semantic columns in reading order with independently wrappable chords", () => {
  const view = render(<ShortcutsModal open onClose={() => {}} />);
  const columns = view.getByRole("dialog").querySelectorAll("[data-shortcut-column]");
  expect(columns).toHaveLength(3);
  for (const [index, column] of Array.from(columns).entries()) {
    expect(Array.from(column.querySelectorAll("section")).map((s) => s.getAttribute("aria-label")))
      .toEqual(shortcutColumns()[index].map((g) => t(g.title)));
    for (const key of column.querySelectorAll("kbd")) {
      expect(key.className).toContain("[overflow-wrap:anywhere]");
      expect(key.parentElement?.className).toContain("max-w-[48%]");
      expect(Array.from(key.querySelectorAll("span")).every((part) => part.className.includes("whitespace-nowrap"))).toBe(true);
      expect(key.querySelectorAll("wbr").length).toBe(key.textContent!.split("/").length - 1);
    }
  }
  expect(view.getAllByRole("button", { name: "Close" }).some((button) => button.textContent === "Close")).toBe(true);
});
