// Decides which lanes a change needs. `npm test` passes the paths that
// differ from HEAD; `npm run test:full` asks for every lane. Kept pure so the
// selection rules are tested directly; scripts/test.mjs gathers the inputs
// and runs the lanes.

import path from "node:path";

// What the type check reads: modules, including the JSON they import, and its
// own configuration (tsconfig*.json, package.json and its lock file).
const TYPE_CHECKED = /\.(?:[cm]?[jt]s|[jt]sx|json)$/;
const RUST_ROOT = "src-tauri/";

/** Markdown outside specs/ is documentation: no test or build reads it. */
function isDocumentation(path) {
  return path.endsWith(".md") && !path.startsWith("specs/");
}

function unique(values) {
  return [...new Set(values)];
}

/**
 * Maps each root Rust test module to the suite target that compiles it, from
 * the `#[path = "../x.rs"] mod x;` lines in tests/suites/<suite>.rs. The
 * target name is `<suite>_test_suite`, which tests/config/rust-test-harnesses
 * enforces.
 *
 * @param {Array<{ suite: string, source: string }>} suites
 * @returns {Map<string, { target: string, module: string }>}
 */
export function rustSuiteModules(suites) {
  const modules = new Map();
  for (const { suite, source } of suites) {
    for (const match of source.matchAll(/#\[path = "\.\.\/([^"]+\.rs)"\]\s*mod (\w+);/g)) {
      modules.set(`${RUST_ROOT}tests/${match[1]}`, {
        target: `${suite}_test_suite`,
        module: match[2],
      });
    }
  }
  return modules;
}

/**
 * The files Rust compiles into the crate through `include_str!`,
 * `include_bytes!`, or `include!`, repository-relative. A change to one is a
 * Rust change wherever it lives, such as a catalogue both halves read.
 *
 * @param {Array<{ file: string, source: string }>} sources repository-relative .rs files
 * @returns {string[]}
 */
export function rustEmbeddedFiles(sources) {
  const files = [];
  for (const { file, source } of sources) {
    for (const match of source.matchAll(/\binclude(?:_str|_bytes)?!\s*\(\s*"([^"]+)"/g)) {
      files.push(path.posix.normalize(path.posix.join(path.posix.dirname(file), match[1])));
    }
  }
  return unique(files);
}

/**
 * A test that reads files through Node instead of importing them. Import-based
 * selection cannot see what such a test depends on, and many read source files
 * as text, so every change but documentation runs every one of them.
 */
export function readsRepository(testSource) {
  return /(?:from\s+|import\s*\(\s*)["'](?:node:)?(?:fs|child_process)(?:\/promises)?["']/.test(testSource);
}

/**
 * @param {object} input
 * @param {string[]} input.changed repository-relative, "/"-separated paths
 * @param {boolean} input.full
 * @param {string} input.platform a `process.platform` value
 * @param {Map<string, { target: string, module: string }>} input.suiteModules
 * @param {string[]} input.repositoryReaders test files for which readsRepository holds
 * @param {string[]} [input.rustInputs] files outside src-tauri/ that Rust embeds, from rustEmbeddedFiles
 */
export function planTests({ changed, full, platform, suiteModules, repositoryReaders, rustInputs = [] }) {
  const onWindows = platform === "win32";
  if (full) {
    return {
      hidden: true,
      specs: true,
      typecheck: true,
      vitest: "all",
      bundle: true,
      rust: "all",
      heavy: true,
      windowsPackaging: onWindows,
    };
  }

  const code = changed.filter((path) => !isDocumentation(path));
  const related = unique([...code, ...(code.length > 0 ? repositoryReaders : [])]);

  const rustPaths = code.filter((path) => path.startsWith(RUST_ROOT) || rustInputs.includes(path));
  const testModules = rustPaths.map((path) => suiteModules.get(path));
  let rust = null;
  if (rustPaths.length > 0) {
    // The library's modules reach one another, so any change outside a root
    // test module can affect every Rust test.
    rust = testModules.every(Boolean)
      ? {
          targets: unique(testModules.map((module) => module.target)),
          filters: unique(testModules.map((module) => `${module.module}::`)),
        }
      : "all";
  }

  return {
    hidden: changed.length > 0,
    specs: code.some((path) => path.startsWith("specs/")),
    typecheck: code.some((path) => TYPE_CHECKED.test(path)),
    vitest: related.length > 0 ? related : null,
    bundle: false,
    rust,
    // The heavy suite's ignored tests are slow and need managed downloads; the
    // ordinary Rust run still compiles them whenever Rust code changes.
    heavy: false,
    windowsPackaging:
      onWindows &&
      code.some((path) => path === "scripts/package.ps1" || path.startsWith("tests/windows/")),
  };
}
