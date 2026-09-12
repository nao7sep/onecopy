import { describe, expect, it } from "vitest";
import { cargoSuiteTargets, cargoTableBody } from "./support/cargo-manifest";

describe.each([
  ["LF", "\n"],
  ["CRLF", "\r\n"],
])("Cargo manifest parsing with %s line endings", (_label, newline) => {
  const manifest = [
    "[lib]",
    'crate-type = ["rlib"]',
    "",
    "[[test]]",
    'name = "library_test_suite"',
    'path = "tests/suites/library.rs"',
    "",
  ].join(newline);

  it("reads a table body", () => {
    expect(cargoTableBody(manifest, "lib")).toContain('crate-type = ["rlib"]');
  });

  it("reads an explicit test target", () => {
    expect(cargoSuiteTargets(manifest)).toEqual([
      { name: "library_test_suite", path: "tests/suites/library.rs" },
    ]);
  });
});
