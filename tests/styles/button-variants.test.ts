import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// A disabled control is visibly faded (interface-styling-conventions). The
// ghost variant used to fail that by restating its resting `text-ink-muted` as
// its disabled colour, so a button that had run out of list looked exactly
// like the live one beside it. The trap is the shape of the variant table
// rather than one variant, so read the table and hold every row to the rule.
const source = readFileSync(
  join(__dirname, "../../src/components/ui/Button.tsx"),
  "utf8",
);

function variantClasses(): Map<string, string> {
  const table = source.match(/const VARIANTS[^{]*\{([\s\S]*?)\n\};/);
  if (table === null) throw new Error("Button.tsx no longer declares a VARIANTS table");
  const variants = new Map<string, string>();
  for (const [, name, classes] of table[1].matchAll(/(\w+):\s*((?:\s*"[^"]*")+)/g)) {
    variants.set(name, [...classes.matchAll(/"([^"]*)"/g)].map(([, part]) => part).join(" "));
  }
  return variants;
}

describe("Button variants", () => {
  const variants = variantClasses();

  it("declares the four variants the primitive types", () => {
    expect([...variants.keys()].sort()).toEqual(["danger", "ghost", "primary", "secondary"]);
  });

  it.each([...variants])("fades %s when it is disabled", (_name, classes) => {
    const utilities = classes.split(/\s+/).filter((utility) => utility.length > 0);
    const resting = new Set(utilities.filter((utility) => !utility.includes(":")));
    const disabled = utilities
      .filter((utility) => utility.startsWith("disabled:"))
      .map((utility) => utility.slice("disabled:".length));

    expect(disabled.length).toBeGreaterThan(0);
    // Every disabled utility must change something. One that restates a
    // resting utility is the defect: the rule looks satisfied and nothing
    // moves on screen.
    expect(disabled.filter((utility) => resting.has(utility))).toEqual([]);
  });

  // The opposite failure, and the one that reached the screen: a disabled
  // utility that changes too much. Primary and danger each replaced their
  // surface with the same neutral fill and the same neutral ink, so off they
  // were one control rather than two, and the destructive one had dropped its
  // red and its outline. A disabled variant may only recede — never restate the
  // fill, outline or ink that make it the variant it is.
  it.each([...variants])("lets %s recede when disabled rather than reskinning it", (_name, classes) => {
    const disabled = classes
      .split(/\s+/)
      .filter((utility) => utility.startsWith("disabled:"))
      .map((utility) => utility.slice("disabled:".length));

    expect(disabled.filter((utility) => /^(bg|text|border)-/.test(utility))).toEqual([]);
  });

  // Off, no variant may be mistaken for another: they recede by one answer, so
  // what tells them apart at rest still tells them apart while they are off.
  it("gives every variant the same disabled answer", () => {
    const answers = new Set(
      [...variants.values()].map((classes) =>
        classes
          .split(/\s+/)
          .filter((utility) => utility.startsWith("disabled:"))
          .sort()
          .join(" "),
      ),
    );
    expect([...answers]).toHaveLength(1);
  });
});
