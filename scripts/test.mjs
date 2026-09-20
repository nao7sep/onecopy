// The two test entry points, as a fixed list of lanes.
//
// `npm test` runs the default set: the hidden-character and spec-structure
// checks, the type check, the frontend bundle, every Vitest test, and every
// Cargo test. `npm run test:full` adds the lanes held out for cost — the heavy
// Rust suite, which downloads and runs the managed tools and models, and on
// Windows the packaging failure script.
//
// Neither run reads the working tree, Git, or a file's timestamp: the same
// lanes run on every invocation, so two runs at one commit are comparable.
// tests/README.md records which areas of OneCopy the default set stands for.

import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const MANIFEST = ["--manifest-path", "src-tauri/Cargo.toml"];
const onWindows = process.platform === "win32";

function run(label, command, args, shell = false) {
  process.stdout.write(`\n› ${label}\n`);
  const result = spawnSync(command, args, { cwd: ROOT, stdio: "inherit", shell });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

function runNode(label, script, args = []) {
  run(label, process.execPath, [path.join(ROOT, script), ...args]);
}

const full = process.argv.includes("--full");

runNode("hidden characters", "scripts/check-hidden-characters.mjs");
runNode("spec structure", "scripts/check-spec-structure.mjs");
// npm is a .cmd shim on Windows, which Node starts only through a shell.
run("typecheck", "npm", ["run", "typecheck"], onWindows);
runNode("frontend bundle", "node_modules/vite/bin/vite.js", ["build"]);
runNode("vitest", "node_modules/vitest/vitest.mjs", ["run"]);
run("cargo test", "cargo", ["test", ...MANIFEST]);

if (full) {
  run("heavy suite: managed tools, models and the shared corpus", "cargo", [
    "test",
    ...MANIFEST,
    "--test",
    "heavy_test_suite",
    "--",
    "--ignored",
  ]);
  if (onWindows) run("Windows package failure handling", "pwsh", ["tests/windows/package-fail-closed.ps1"]);
}
