import { describe, expect, it } from "vitest";
import { shortcutColumns, shortcutGroups } from "../../src/models/shortcuts";
import { primaryModWord } from "../../src/utils/shortcuts";
import { t } from "../helpers/i18n";

// Catalogue assertions cover authored scope and organization. Actual dispatch
// is exercised by the grid, tree, viewer, Comparison, modal and command suites;
// finding a key's spelling somewhere in source cannot prove that it works.
describe("shortcut catalogue", () => {
  it("keeps related contexts in three balanced reading columns", () => {
    const columns = shortcutColumns();
    expect(columns.map((column) => column.map((group) => t(group.title)))).toEqual([
      ["Main items", "Sections", "Destinations"],
      ["Fullscreen view", "Preview window", "Media and text", "Confirmations"],
      ["Comparison", "App"],
    ]);
    // A column is as tall as its rows plus a heading and context line per group.
    const lengths = columns.map((column) => column.reduce((n, group) => n + group.rows.length + 2, 0));
    expect(Math.max(...lengths) - Math.min(...lengths)).toBeLessThanOrEqual(4);
    expect(columns.flat()).toEqual(shortcutGroups());
  });

  it("names keycaps consistently and gives every group an honest context", () => {
    for (const group of shortcutGroups()) {
      expect(t(group.context)).not.toContain("anywhere");
      expect(t(group.context).length).toBeGreaterThan(0);
      for (const row of group.rows) {
        expect(row.chord).not.toMatch(/Cmd\/Ctrl|Ctrl\/Cmd|Page Up|Page Down|PgUp|PgDn|⌘/);
        expect(row.chord).not.toMatch(primaryModWord() === "Cmd" ? /Ctrl\+/ : /Cmd\+/);
      }
    }
  });

  it("distinguishes selection scope, keep decisions, and viewer round trips", () => {
    const groups = shortcutGroups();
    const row = (title: string, chord: string) => {
      const action = groups
        .find((group) => t(group.title) === title)
        ?.rows.find((r) => r.chord === chord)?.action;
      return action === undefined ? undefined : t(action);
    };
    expect(row("Main items", "Delete/Backspace")).toContain("selected items");
    expect(row("Preview window", "Delete/Backspace")).toContain("complete selection");
    expect(row("Fullscreen view", "Delete/Backspace")).toContain("only the displayed item");
    expect(row("Fullscreen view", "Space/Escape")).toBe("Return to Main");
    expect(row("Main items", "F")).toBeUndefined();
    expect(row("Comparison", "Delete/Backspace")).toContain("marked images themselves");
    expect(row("Comparison", "Enter")).toContain("unmarked images");
    expect(row("Comparison", "Space")).toBeUndefined();
    expect(row("Preview window", "F")).toBeUndefined();
    expect(row("Preview window", "Escape")).toBe("Close the preview window");
    expect(row("Destinations", "Enter")).toContain("Expand/collapse only");
  });
});
