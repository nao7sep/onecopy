// Decides which checks a change needs. `npm run check` passes the paths that
// differ from HEAD; `npm run check:full` asks for every lane. Kept pure so the
// selection rules are tested directly; scripts/check.mjs gathers the inputs
// and runs the lanes.

const TYPESCRIPT = /\.(ts|tsx)$/;
const TYPESCRIPT_CONFIG = /^(tsconfig[^/]*\.json|package(-lock)?\.json)$/;
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
 * A test that reads repository files through Node instead of importing them.
 * Import-based selection cannot see what such a test depends on, so any change
 * to a file outside the TypeScript module graph runs every one of them.
 */
export function readsRepository(testSource) {
  return /from\s+"node:(fs|child_process)(\/promises)?"/.test(testSource);
}

/**
 * @param {object} input
 * @param {string[]} input.changed repository-relative, "/"-separated paths
 * @param {boolean} input.full
 * @param {string} input.platform a `process.platform` value
 * @param {Map<string, { target: string, module: string }>} input.suiteModules
 * @param {string[]} input.repositoryReaders test files for which readsRepository holds
 */
export function planChecks({ changed, full, platform, suiteModules, repositoryReaders }) {
  const onWindows = platform === "win32";
  if (full) {
    return {
      hidden: true,
      specs: true,
      typecheck: true,
      vitest: "all",
      bundle: true,
      rust: "all",
      windowsPackaging: onWindows,
    };
  }

  const code = changed.filter((path) => !isDocumentation(path));
  const outsideModuleGraph = code.some((path) => !TYPESCRIPT.test(path));
  const related = unique([...code, ...(outsideModuleGraph ? repositoryReaders : [])]);

  const rustPaths = code.filter((path) => path.startsWith(RUST_ROOT));
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
    typecheck: code.some((path) => TYPESCRIPT.test(path) || TYPESCRIPT_CONFIG.test(path)),
    vitest: related.length > 0 ? related : null,
    bundle: false,
    rust,
    windowsPackaging:
      onWindows &&
      code.some((path) => path === "scripts/package.ps1" || path.startsWith("tests/windows/")),
  };
}
