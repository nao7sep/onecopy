import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// A label beside its value may wrap (localization-conventions, "Translated
// text remains readable"); the value keeps the rest of the row. The label
// column takes at most 40% of a narrow pane, so a long translated key wraps
// instead of squeezing the value into a letter-wide column. Shortcut keys are
// control captions and stay on one line. This is a source-level regression
// gate for those bounds, not proof of rendered fit, which the developer's
// visual check covers.

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
    container: "grid grid-cols-[fit-content(40%)_minmax(0,1fr)]",
    label: 'className="break-words text-ink-muted">{row.label}',
  },
  {
    file: "TextOrAttributesSurface.tsx",
    describe: "the Kind/Size/Date/Copies attribute rows",
    container: "grid grid-cols-[fit-content(40%)_minmax(0,1fr)]",
    label: 'className="break-words text-ink-muted">{t("common.kind")}',
  },
  {
    file: "ShortcutsModal.tsx",
    describe: "the shortcut-key label beside its description",
    container: "max-w-[48%] shrink-0",
    label: "whitespace-nowrap",
  },
];

describe("label/value row layout", () => {
  it.each(ROWS)("$file bounds the label column for $describe", ({ file, container, label }) => {
    const source = read(file);
    expect(source.includes(container), `${file}: expected the row container to include ${JSON.stringify(container)}`).toBe(true);
    expect(source.includes(label), `${file}: expected a label to include ${JSON.stringify(label)}`).toBe(true);
  });
});
