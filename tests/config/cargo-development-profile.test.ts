import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const manifest = readFileSync("src-tauri/Cargo.toml", "utf8");

function profileSection(name: string): string {
  const heading = `[${name}]\n`;
  const start = manifest.indexOf(heading);
  expect(start, `Cargo profile section [${name}]`).toBeGreaterThanOrEqual(0);
  const body = manifest.slice(start + heading.length);
  const next = body.search(/^\[/m);
  return next < 0 ? body : body.slice(0, next);
}

describe("the Cargo development build policy", () => {
  it("emits only the desktop library artifact", () => {
    expect(profileSection("lib")).toMatch(/^crate-type\s*=\s*\["rlib"\]\s*$/m);
  });

  it("keeps lightweight source-level application diagnostics", () => {
    for (const profile of ["dev", "test"]) {
      expect(profileSection(`profile.${profile}`)).toMatch(
        /debug\s*=\s*"line-tables-only"/,
      );
      expect(profileSection(`profile.${profile}`)).toMatch(
        /split-debuginfo\s*=\s*"off"/,
      );
      expect(profileSection(`profile.${profile}.package."*"`)).toMatch(
        /debug\s*=\s*false/,
      );
      expect(profileSection(`profile.${profile}.package."*"`)).not.toMatch(
        /opt-level/,
      );
    }
  });

  it("keeps full debugging explicit and opt-in", () => {
    expect(profileSection("profile.debugging")).toMatch(/inherits\s*=\s*"dev"/);
    expect(profileSection("profile.debugging")).toMatch(/debug\s*=\s*true/);
  });

  it("retains optimization only for measured CPU-heavy dependencies", () => {
    const required = [
      "image",
      "webp",
      "libwebp-sys",
      "blake3",
      "sha2",
      "whisper-rs",
      "whisper-rs-sys",
      "ort",
      "ort-sys",
      "rusqlite",
      "libsqlite3-sys",
      "zstd",
      "zstd-sys",
    ];
    for (const dependency of required) {
      expect(profileSection(`profile.dev.package.${dependency}`)).toMatch(/opt-level\s*=\s*2/);
    }
  });
});
