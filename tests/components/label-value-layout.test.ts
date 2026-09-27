import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// localization-conventions: a label in a label-and-value row must never wrap.
// The label column is sized to its longest label (CSS grid, max-content
// column) with the label itself set to nowrap; the value wraps instead. This
// is a source-level regression gate rather than a render test because the
// failure mode is a CSS regression (someone drops `whitespace-nowrap` or
// widens the grid track), not a logic bug — grepping the JSX for the pattern
// catches that just as reliably as measuring layout, without needing to
// render every surface in every language.

const SRC = join(process.cwd(), "src/components");

function read(file: string): string {
  return readFileSync(join(SRC, file), "utf8");
}

// Every label-and-value row this app renders side by side (not stacked one
// above the other, where wrapping can't collide with the value). Each entry
// names the file, a short description of the row, and the exact snippet the
// label markup must contain.
const ROWS: Array<{ file: string; describe: string; container: string; label: string }> = [
  {
    file: "MetadataPane.tsx",
    describe: "the per-item background-work status rows in the Details pane",
    container: "grid grid-cols-[max-content_minmax(0,1fr)]",
    label: 'className="whitespace-nowrap text-ink-muted">{row.label}',
  },
  {
    file: "TextOrAttributesSurface.tsx",
    describe: "the Kind/Size/Date/Copies attribute rows",
    container: "grid grid-cols-[max-content_minmax(0,1fr)]",
    label: 'className="whitespace-nowrap text-ink-muted">{t("common.kind")}',
  },
  {
    file: "ActivityTraceModal.tsx",
    describe: "the Session/Operation/Caused by/Events detail rows",
    container: "grid grid-cols-[max-content_minmax(0,1fr)]",
    label: 'className="whitespace-nowrap">{t("activity.session")}',
  },
  {
    file: "ShortcutsModal.tsx",
    describe: "the shortcut-key label beside its description",
    container: "max-w-[48%] shrink-0",
    label: "whitespace-nowrap",
  },
];

describe("label/value row layout", () => {
  it.each(ROWS)("$file keeps a nowrap, max-content label column for $describe", ({ file, container, label }) => {
    const source = read(file);
    expect(source.includes(container), `${file}: expected the row container to include ${JSON.stringify(container)}`).toBe(true);
    expect(source.includes(label), `${file}: expected a label to include ${JSON.stringify(label)}`).toBe(true);
  });
});
