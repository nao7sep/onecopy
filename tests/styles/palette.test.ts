import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { contrast, resolveRgb, themeBlock, type Rgb } from "../helpers/themeCss";

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
  ["--ink-inverted", "--danger-solid"],
  ["--ink-inverted", "--danger-solid-hover"],
  ["--danger", "--surface"],
  ["--danger", "--danger-surface"],
  ["--warning", "--surface"],
  ["--warning", "--warning-surface"],
];

// Boundaries that alone identify a control or its state: a form field's
// outline against the field and its surroundings, and the focus ring.
const BOUNDARY_PAIRS: ReadonlyArray<[string, string]> = [
  ["--input-border", "--surface"],
  ["--input-border", "--background"],
  ["--primary-ring", "--surface"],
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
