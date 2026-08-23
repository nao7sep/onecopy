// Pure parsing helpers for the version-consistency test.
//
// These are kept free of file I/O so their edge cases — TOML quoting, the
// [package] table isolation, the Cargo.lock [[package]] block isolation,
// inline comments, CRLF — can be unit-tested against synthetic strings
// without touching the real manifests. tests/version.test.ts supplies the I/O
// by reading the actual files.

// Semantic Versioning 2.0.0: major.minor.patch with optional -prerelease and
// +build metadata. The consistency test only asserts the manifests AGREE and
// that the canonical version is well-formed semver; it deliberately does not try
// to be a bundler-compatibility linter, so prerelease/build forms are accepted.
export const SEMVER =
  /^\d+\.\d+\.\d+(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;

export function parseJsonVersion(jsonText: string): string {
  const version = JSON.parse(jsonText).version;
  if (typeof version !== "string" || version.length === 0) {
    throw new Error('Missing string "version"');
  }
  return version;
}

export function parseCargoPackageVersion(tomlText: string): string {
  // Isolate the [package] table so a dependency's own `version = "..."` in a
  // later table cannot be matched. `\[package\]` requires the literal closing
  // bracket, so it never matches `[package.metadata]` and friends.
  const table = /\[package\]([\s\S]*?)(?:\n\[|$)/.exec(tomlText);
  if (!table) {
    throw new Error("No [package] table");
  }
  // TOML strings are either basic ("...") or literal ('...'); accept both. A
  // trailing inline comment after the closing quote is ignored by the anchor.
  const match = /^[ \t]*version[ \t]*=[ \t]*["']([^"']+)["']/m.exec(table[1]);
  if (!match) {
    throw new Error("No version in the [package] table");
  }
  return match[1];
}

// Cargo.lock lists every crate in the dependency graph as its own [[package]]
// block, sorted by name — including the app's own crate alongside every
// registry dependency. A dependency can easily share the app's literal version
// number (e.g. "0.1.0" is common for pre-1.0 crates), so the block is isolated
// by its `name` line first, and only then is `version` read out of that same
// block — exactly mirroring how parseCargoPackageVersion isolates [package]
// from [package.metadata] in Cargo.toml.
export function parseCargoLockPackageVersion(
  lockText: string,
  packageName: string,
): string {
  const blocks = lockText.split(/\n(?=\[\[package\]\])/);
  for (const block of blocks) {
    const nameMatch = /^[ \t]*name[ \t]*=[ \t]*"([^"]+)"/m.exec(block);
    if (!nameMatch || nameMatch[1] !== packageName) continue;
    const versionMatch = /^[ \t]*version[ \t]*=[ \t]*"([^"]+)"/m.exec(block);
    if (!versionMatch) {
      throw new Error(`No version in the [[package]] "${packageName}" block`);
    }
    return versionMatch[1];
  }
  throw new Error(`No [[package]] block named "${packageName}"`);
}
