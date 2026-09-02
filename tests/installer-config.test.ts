import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

describe("Windows installer configuration", () => {
  it("allows the user to choose current-user or all-users installation", () => {
    const config = JSON.parse(
      readFileSync(join(process.cwd(), "src-tauri", "tauri.conf.json"), "utf8"),
    );

    expect(config.bundle?.windows?.nsis?.installMode).toBe("both");
  });
});
