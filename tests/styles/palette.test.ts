import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const css = readFileSync(join(__dirname, "../../src/App.css"), "utf8");

describe("semantic palette contrast", () => {
  it("uses the readable dark danger text tier without changing solid buttons or light mode", () => {
    const light = /:root\s*{([\s\S]*?)}\s*\.dark\s*{/.exec(css)?.[1];
    const dark = /\.dark\s*{([\s\S]*?)}\s*@theme/.exec(css)?.[1];

    expect(light).toMatch(/--danger:\s*var\(--color-red-600\)/);
    expect(light).toMatch(/--danger-surface:\s*var\(--color-red-50\)/);
    expect(dark).toMatch(/--danger:\s*var\(--color-red-300\)/);
    expect(dark).toMatch(/--danger-surface:\s*var\(--color-red-900\)/);
    expect(dark).toMatch(/--danger-solid:\s*var\(--color-red-600\)/);
    expect(dark).toMatch(/--danger-solid-hover:\s*var\(--color-red-500\)/);
  });
});
