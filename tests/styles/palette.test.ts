import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { contrast, luminance, resolveRgb, themeBlock, tokenValue, type Rgb } from "../helpers/themeCss";

const modalShellSource = readFileSync(
  join(__dirname, "../../src/components/ModalShell.tsx"),
  "utf8",
);
const fieldSource = readFileSync(join(__dirname, "../../src/components/ui/Field.tsx"), "utf8");

const css = readFileSync(join(__dirname, "../../src/App.css"), "utf8");
const tailwindTheme = readFileSync(
  join(__dirname, "../../node_modules/tailwindcss/theme.css"),
  "utf8",
);
const themeRs = readFileSync(join(__dirname, "../../src-tauri/src/theme.rs"), "utf8");
const sources = `${css}\n${tailwindTheme}`;
const themes = ["light", "dark"] as const;

// Foreground tokens on the surfaces the components place them on: ink on the
// neutral surfaces, accent text and links, inverted text on filled Primary and
// Danger buttons, and danger and warning text on their own washes.
const TEXT_PAIRS: ReadonlyArray<[string, string]> = [
  ...["--ink-strong", "--ink", "--ink-muted"].flatMap(
    (ink): Array<[string, string]> => [
      [ink, "--background"],
      [ink, "--surface"],
      [ink, "--surface-muted"],
    ],
  ),
  ["--primary", "--surface"],
  ["--primary", "--background"],
  ["--primary", "--primary-surface"],
  ["--primary-hover", "--surface"],
  ["--ink-inverted", "--primary"],
  ["--ink-inverted", "--primary-hover"],
  ["--ink-inverted", "--primary-pressed"],
  ["--ink-inverted", "--danger-solid"],
  ["--ink-inverted", "--danger-solid-hover"],
  ["--ink-inverted", "--danger-solid-pressed"],
  ["--danger", "--surface"],
  ["--danger", "--danger-surface"],
  ["--warning", "--surface"],
  ["--warning", "--warning-surface"],
];

// The focus ring alone shows where the keyboard is. Edges are lines, held to
// their kinds' ranges below rather than to a legibility floor
// (interface-styling-conventions, "What decides").
const BOUNDARY_PAIRS: ReadonlyArray<[string, string]> = [
  ["--primary-ring", "--surface"],
  ["--primary-ring", "--background"],
];

function pairContrast(theme: (typeof themes)[number], foreground: string, background: string): number {
  const block = themeBlock(css, theme);
  return contrast(resolveRgb(block, foreground, sources), resolveRgb(block, background, sources));
}

describe("semantic palette contrast", () => {
  for (const theme of themes) {
    it(`keeps text at 4.5:1 or more in the ${theme} theme`, () => {
      for (const [foreground, background] of TEXT_PAIRS) {
        expect(pairContrast(theme, foreground, background), `${foreground} on ${background}`)
          .toBeGreaterThanOrEqual(4.5);
      }
    });

    it(`keeps control boundaries at 3:1 or more in the ${theme} theme`, () => {
      for (const [foreground, background] of BOUNDARY_PAIRS) {
        expect(pairContrast(theme, foreground, background), `${foreground} on ${background}`)
          .toBeGreaterThanOrEqual(3);
      }
    });
  }
});

// The line kinds (interface-styling-conventions, "One strength per kind of
// line"), each against the surfaces the app paints it on: the panes and
// modals (--surface) and Main's canvas (--background). Row separators sit
// only between rows inside modals. These record the values chosen for each
// kind, so a later pass cannot drift a line out of its kind's range.
const LINE_KINDS: ReadonlyArray<[string, string, readonly string[], number, number]> = [
  ["row separator", "--border-subtle", ["--surface"], 1.15, 1.3],
  ["section divider, band edge and card edge", "--border", ["--surface", "--background"], 1.35, 1.7],
  ["control edge", "--control-edge", ["--surface", "--background"], 2.0, 2.5],
];

describe("line kinds", () => {
  for (const theme of themes) {
    it(`keeps every line kind within its range in the ${theme} theme`, () => {
      for (const [kind, token, surfaces, floor, ceiling] of LINE_KINDS) {
        for (const surface of surfaces) {
          const ratio = pairContrast(theme, token, surface);
          expect(ratio, `${kind} (${token}) on ${surface}`).toBeGreaterThanOrEqual(floor);
          expect(ratio, `${kind} (${token}) on ${surface}`).toBeLessThanOrEqual(ceiling);
        }
      }
    });

    // Rest, hover and press move one way (interface-styling-conventions, "A
    // ladder moves one way"): each step lies further from the resting fill
    // than the last, in the same direction.
    it(`steps every filled and neutral control one way in the ${theme} theme`, () => {
      const block = themeBlock(css, theme);
      const lightness = (token: string) => luminance(resolveRgb(block, token, sources));
      for (const ladder of [
        ["--primary", "--primary-hover", "--primary-pressed"],
        ["--danger-solid", "--danger-solid-hover", "--danger-solid-pressed"],
        ["--surface", "--surface-muted", "--surface-pressed"],
      ]) {
        const [rest, hover, pressed] = ladder.map(lightness) as [number, number, number];
        const direction = Math.sign(hover - rest);
        expect(direction, ladder.join(" → ")).not.toBe(0);
        expect(Math.sign(pressed - hover), ladder.join(" → ")).toBe(direction);
      }
    });
  }
});

// One focus treatment, quiet while the window is inactive
// (interface-styling-conventions, States): the ring is the primary ring, and
// the inactive window re-points it at the control edge, outside both theme
// blocks so it holds in either.
describe("focus ring", () => {
  it("is the primary ring at rest and the control edge in an inactive window", () => {
    expect(tokenValue(themeBlock(css, "light"), "--focus-ring")).toBe("var(--primary-ring)");
    const inactive = css.match(/:root\[data-window-inactive\]\s*\{([^}]*)\}/)?.[1];
    expect(inactive, "inactive-window block").toBeDefined();
    expect(tokenValue(inactive!, "--focus-ring")).toBe("var(--control-edge)");
  });
});

// A `border-<token>` Tailwind utility resolves to the `--<token>` custom
// property (App.css's `@theme inline` block copies every `--color-<token>`
// straight from `--<token>`), so pulling the token name out of the class
// string and reading it as a CSS variable gives the exact colour the
// component paints.
function borderToken(source: string, utilityPattern: RegExp): string {
  const match = source.match(utilityPattern);
  if (!match) throw new Error(`no match for ${utilityPattern} in source`);
  return `--${match[1]}`;
}

describe("modal-dialog band lines", () => {
  // modal-dialog-conventions, "Each band is drawn as a band": the header- and
  // footer-closing lines are "never fainter than the app's own control
  // borders in either theme" — a comparison against whatever token a real
  // control's edge uses, not a fixed ratio. Field.tsx's input is that
  // control: its outline is its only boundary (see the BOUNDARY_PAIRS
  // comment above), while a button's border is decorative chrome on a filled
  // shape. Reading both tokens from source, rather than assuming they are
  // the same variable, means this fails the moment either one drifts.
  const lineToken = borderToken(modalShellSource, /border-b border-(control-edge|border)\b/);
  const controlToken = borderToken(fieldSource, /border-(control-edge|border)"/);

  for (const theme of themes) {
    it(`keeps the header/footer line at least as visible as the input's own border in the ${theme} theme`, () => {
      const block = themeBlock(css, theme);
      const surface = resolveRgb(block, "--surface", sources);
      const lineContrast = contrast(resolveRgb(block, lineToken, sources), surface);
      const controlContrast = contrast(resolveRgb(block, controlToken, sources), surface);
      expect(lineContrast, `${lineToken} vs ${controlToken} on --surface`)
        .toBeGreaterThanOrEqual(controlContrast);
    });
  }
});

describe("native window background", () => {
  function rustBackground(arm: string): Rgb {
    const match = themeRs.match(
      new RegExp(`${arm} => Color\\(0x([0-9a-f]{2}), 0x([0-9a-f]{2}), 0x([0-9a-f]{2}), 0xff\\)`, "i"),
    );
    if (!match) throw new Error(`window_background arm ${arm} missing`);
    return [match[1]!, match[2]!, match[3]!].map((part) => Number.parseInt(part, 16)) as Rgb;
  }

  it("matches --background in each theme so the frame behind each page never flashes", () => {
    for (const [theme, arm] of [["dark", "Theme::Dark"], ["light", "_"]] as const) {
      const expected = resolveRgb(themeBlock(css, theme), "--background", sources);
      rustBackground(arm).forEach((channel, index) => {
        expect(Math.abs(channel - expected[index]!), `${theme} channel ${index}`).toBeLessThanOrEqual(1);
      });
    }
  });
});
