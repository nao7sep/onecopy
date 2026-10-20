// Release gate: `npm run check:release-version`, run by the release workflow.
// On a tag build (GITHUB_REF_TYPE=tag) it fails unless the tag is `v` plus the
// app version in src-tauri/tauri.conf.json; on any other build it only checks
// that a version exists, so the same step runs on every workflow trigger.

import { readFileSync } from "node:fs";

const configUrl = new URL("../src-tauri/tauri.conf.json", import.meta.url);
const config = JSON.parse(readFileSync(configUrl, "utf8"));
const version = config.version;

if (typeof version !== "string" || version.trim() === "") {
  throw new Error("src-tauri/tauri.conf.json has no app version");
}

if (process.env.GITHUB_REF_TYPE === "tag") {
  const expected = `v${version}`;
  const actual = process.env.GITHUB_REF_NAME ?? "";
  if (actual !== expected) {
    process.stderr.write(
      `Release tag ${JSON.stringify(actual)} must equal ${JSON.stringify(expected)}.\n`,
    );
    process.exitCode = 1;
  } else {
    process.stdout.write(`Release tag matches the Tauri version: ${actual}\n`);
  }
} else {
  process.stdout.write("No release tag; version/tag gate skipped.\n");
}
