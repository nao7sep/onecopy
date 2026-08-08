import { readFileSync } from "node:fs";
import { defineConfig } from "vitest/config";

// vitest bypasses vite.config.ts, so the __APP_VERSION__ define is duplicated
// here from the same single source (src-tauri/tauri.conf.json).
const appVersion = (
  JSON.parse(readFileSync("./src-tauri/tauri.conf.json", "utf8")) as {
    version: string;
  }
).version;

export default defineConfig({
  define: {
    __APP_VERSION__: JSON.stringify(appVersion),
  },
  // Default environment is `node` since the bulk of the suite is pure logic in
  // src/services and src/utils; specs that touch the DOM opt into happy-dom
  // with a per-file `// @vitest-environment happy-dom` comment.
  test: {
    environment: "node",
    setupFiles: ["./tests/setup.ts"],
    include: ["tests/**/*.test.ts"],
    coverage: {
      // V8's native coverage for the frontend (the Rust backend has its own
      // cargo-llvm-cov pass). `include` spans src so the report flags logic no
      // test reaches, not just a score for what is reached.
      provider: "v8" as const,
      reporter: ["text", "html", "lcov"],
      include: ["src/**/*.{ts,tsx}"],
      // Excluded as framework wiring with no decision to cover:
      exclude: [
        "src/main.tsx", // React DOM mount
        "src/vite-env.d.ts",
        "**/*.d.ts",
      ],
    },
  },
});
