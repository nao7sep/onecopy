// `npm run check` runs only the checks that the working-tree changes against
// HEAD can affect; `npm run check:full` runs every check. check-plan.mjs owns
// the selection; this file gathers its inputs and runs the chosen lanes in
// order, stopping at the first failure.

import { execFileSync, spawnSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { planChecks, readsRepository, rustSuiteModules } from "./check-plan.mjs";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const SUITES = path.join(ROOT, "src-tauri", "tests", "suites");
const MANIFEST = ["--manifest-path", "src-tauri/Cargo.toml"];
const onWindows = process.platform === "win32";

function changedPaths() {
  const status = execFileSync(
    "git",
    ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    { cwd: ROOT, encoding: "utf8" },
  );
  const entries = status.split("\0").filter(Boolean);
  const paths = [];
  for (let index = 0; index < entries.length; index += 1) {
    const state = entries[index].slice(0, 2);
    paths.push(entries[index].slice(3));
    // With -z, a rename or copy is followed by its source path.
    if (state[0] === "R" || state[0] === "C") paths.push(entries[(index += 1)]);
  }
  return paths;
}

function testFiles(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const absolute = path.join(directory, entry.name);
    if (entry.isDirectory()) return testFiles(absolute);
    return /\.test\.tsx?$/.test(entry.name) ? [absolute] : [];
  });
}

function repositoryPath(absolute) {
  return path.relative(ROOT, absolute).split(path.sep).join("/");
}

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
const changed = full ? [] : changedPaths();
const plan = planChecks({
  changed,
  full,
  platform: process.platform,
  suiteModules: rustSuiteModules(
    readdirSync(SUITES)
      .filter((name) => name.endsWith(".rs"))
      .map((name) => ({
        suite: path.basename(name, ".rs"),
        source: readFileSync(path.join(SUITES, name), "utf8"),
      })),
  ),
  repositoryReaders: testFiles(path.join(ROOT, "tests"))
    .filter((file) => readsRepository(readFileSync(file, "utf8")))
    .map(repositoryPath),
});

if (!full) {
  process.stdout.write(
    changed.length === 0
      ? "Nothing differs from HEAD; no checks to run.\n"
      : `Checking ${changed.length} changed path(s) against HEAD.\n`,
  );
}

if (plan.hidden) runNode("hidden characters", "scripts/check-hidden-characters.mjs");
if (plan.specs) runNode("spec structure", "scripts/check-spec-structure.mjs");
// npm is a .cmd shim on Windows, which Node starts only through a shell.
if (plan.typecheck) run("typecheck", "npm", ["run", "typecheck"], onWindows);
if (plan.vitest === "all") {
  runNode("vitest", "node_modules/vitest/vitest.mjs", ["run"]);
} else if (plan.vitest) {
  runNode("vitest related", "node_modules/vitest/vitest.mjs", [
    "related",
    "--run",
    "--passWithNoTests",
    ...plan.vitest,
  ]);
}
if (plan.bundle) runNode("frontend bundle", "node_modules/vite/bin/vite.js", ["build"]);
if (plan.rust === "all") {
  run("cargo test", "cargo", ["test", ...MANIFEST]);
} else if (plan.rust) {
  run(`cargo test ${plan.rust.filters.join(" ")}`, "cargo", [
    "test",
    ...MANIFEST,
    ...plan.rust.targets.flatMap((target) => ["--test", target]),
    "--",
    ...plan.rust.filters,
  ]);
}
if (plan.windowsPackaging) {
  run("Windows package failure handling", "pwsh", ["tests/windows/package-fail-closed.ps1"]);
}
