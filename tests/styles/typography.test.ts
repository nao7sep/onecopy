import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const shippedTextSurfaces = [
  "../../src/components/MetadataPane.tsx",
  "../../src/components/PreviewSurface.tsx",
];

describe("tertiary media labels", () => {
  it("keep readable snapshot timestamps at the fleet floor", () => {
    for (const relative of shippedTextSurfaces) {
      const source = readFileSync(join(__dirname, relative), "utf8");
      expect(source).not.toContain("text-[10px]");
      expect(source).toMatch(/timestampLabel[\s\S]{0,900}text-\[11px\]/);
    }
  });
});
