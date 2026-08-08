import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  SEMVER,
  parseCargoLockPackageVersion,
  parseCargoPackageVersion,
  parseJsonVersion,
} from "./helpers/versions";

// The app version is declared in four manifests, each of which its own tool
// requires to carry a literal value:
//
//   - src-tauri/tauri.conf.json  the version the bundle/installer carries and
//     the OS reports. It is injected as __APP_VERSION__ (see vite.config.ts /
//     vitest.config.ts) and is what the release workflow names the installer
//     from. This is the canonical source of truth.
//   - package.json               the npm manifest (private; never published).
//   - src-tauri/Cargo.toml       the Rust crate version.
//   - src-tauri/Cargo.lock       the lockfile's own [[package]] entry for the
//     "onecopy" crate (isolated from every other crate's [[package]] block,
//     since a dependency can easily share the same literal version number).
//
// Every user-facing surface derives from tauri.conf.json, so the other three
// only have to stay equal to it. This file is the guard that they do; a drift
// fails the suite and blocks the release before any installer is built.
//
// Each version is read inside its own test so a single malformed/missing file
// fails just that case cleanly, instead of throwing during collection and taking
// the unrelated checks down with it.

const TAURI_CONF = "src-tauri/tauri.conf.json";

// `npm test` runs vitest from the repo root, so cwd is the project root.
function read(relativePath: string): string {
  return readFileSync(join(process.cwd(), relativePath), "utf8");
}

function canonicalVersion(): string {
  return parseJsonVersion(read(TAURI_CONF));
}

describe("app version is consistent across manifests", () => {
  it("tauri.conf.json carries a valid semver version", () => {
    expect(canonicalVersion()).toMatch(SEMVER);
  });

  it("package.json matches the canonical version", () => {
    expect(parseJsonVersion(read("package.json"))).toBe(canonicalVersion());
  });

  it("package-lock.json matches the canonical version in both places", () => {
    const canonical = canonicalVersion();
    const lock = JSON.parse(read("package-lock.json"));
    expect(lock.version).toBe(canonical);
    expect(lock.packages?.[""]?.version).toBe(canonical);
  });

  it("src-tauri/Cargo.toml matches the canonical version", () => {
    expect(parseCargoPackageVersion(read("src-tauri/Cargo.toml"))).toBe(canonicalVersion());
  });

  it("src-tauri/Cargo.lock matches the canonical version for the onecopy package", () => {
    const lockVersion = parseCargoLockPackageVersion(read("src-tauri/Cargo.lock"), "onecopy");
    expect(lockVersion).toBe(canonicalVersion());
  });
});
