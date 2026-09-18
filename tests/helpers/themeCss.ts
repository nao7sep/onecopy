// Pure helpers for reading App.css's theme token blocks and measuring WCAG
// contrast. The light tokens live in the top-level :root block; the dark tokens
// live in the :root block inside @media (prefers-color-scheme: dark). Both alias
// Tailwind's palette, so `var(--color-*)` references resolve through Tailwind's
// theme.css, whose colors are OKLCH.

export type Rgb = [number, number, number];

export function themeBlock(css: string, theme: "light" | "dark"): string {
  if (theme === "light") {
    const start = css.search(/^:root\s*\{/m);
    if (start < 0) throw new Error("top-level :root block missing");
    return css.slice(css.indexOf("{", start), css.indexOf("\n}", start));
  }
  const media = css.indexOf("@media (prefers-color-scheme: dark) {");
  if (media < 0) throw new Error("prefers-color-scheme: dark block missing");
  const start = css.indexOf("  :root {", media);
  if (start < 0) throw new Error(":root block missing inside the dark media query");
  return css.slice(css.indexOf("{", start), css.indexOf("\n  }", start));
}

export function tokenValue(source: string, token: string): string {
  const match = source.match(new RegExp(`${token.replaceAll("-", "\\-")}\\s*:\\s*([^;]+);`));
  if (!match) throw new Error(`${token} must be defined`);
  return match[1]!.replace(/\/\*.*?\*\//g, "").trim();
}

// Resolves a token in `block` to sRGB (0–255 channels), following var()
// references through `sources` (App.css plus Tailwind's theme.css).
export function resolveRgb(block: string, token: string, sources: string): Rgb {
  let value = tokenValue(block, token);
  for (let depth = 0; depth < 4; depth += 1) {
    const reference = value.match(/^var\((--[^)]+)\)$/)?.[1];
    if (reference === undefined) break;
    value = tokenValue(sources, reference);
  }
  const hex = value.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i)?.[1];
  if (hex !== undefined) {
    const expanded = hex.length === 3
      ? [...hex].map((part) => `${part}${part}`).join("")
      : hex;
    return [0, 2, 4].map(
      (offset) => Number.parseInt(expanded.slice(offset, offset + 2), 16),
    ) as Rgb;
  }

  const oklch = value.match(/^oklch\(([\d.]+)%\s+([\d.]+)\s+([\d.]+)\)$/);
  if (!oklch) throw new Error(`${token} must resolve to an opaque hex or OKLCH color`);
  const lightness = Number(oklch[1]) / 100;
  const chroma = Number(oklch[2]);
  const hue = Number(oklch[3]) * Math.PI / 180;
  const a = chroma * Math.cos(hue);
  const b = chroma * Math.sin(hue);
  const l = (lightness + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (lightness - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (lightness - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const linear = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
  return linear.map((channel) => {
    const clipped = Math.min(1, Math.max(0, channel));
    const srgb = clipped <= 0.0031308
      ? 12.92 * clipped
      : 1.055 * clipped ** (1 / 2.4) - 0.055;
    return srgb * 255;
  }) as Rgb;
}

export function luminance(rgb: Rgb): number {
  const [red, green, blue] = rgb.map((channel) => {
    const value = channel / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * red! + 0.7152 * green! + 0.0722 * blue!;
}

export function contrast(first: Rgb, second: Rgb): number {
  const a = luminance(first);
  const b = luminance(second);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}
