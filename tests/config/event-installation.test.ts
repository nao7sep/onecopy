import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const root = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const read = (path: string) => readFileSync(resolve(root, path), "utf8");

const multiListenerOwners = [
  "src/state/binaries-store.ts",
  "src/state/derived-work-store.ts",
  "src/state/transcript-store.ts",
  "src/workflows/comparison.ts",
  "src/workflows/mutation-events.ts",
  "src/workflows/scan-events.ts",
] as const;

describe("app-lifetime event installation", () => {
  it("keeps migrated multi-listener owners behind the shared transaction", () => {
    for (const path of multiListenerOwners) {
      const source = read(path);
      expect(source.includes("createEventInstaller"), path).toBe(true);
    }
  });

  it("admits managed-tool and transcript listeners from Ready bootstrap", () => {
    for (const path of [
      "src/state/binaries-store.ts",
      "src/state/transcript-store.ts",
    ]) {
      expect(read(path), path).not.toContain("void (async () =>");
    }
    const bootstrap = read("src/workflows/app-lifecycle.ts");
    expect(bootstrap).toContain("installBinariesEventWiring()");
    expect(bootstrap).toContain("installTranscriptEventWiring()");
  });
});
