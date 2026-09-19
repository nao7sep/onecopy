import { describe, expect, it } from "vitest";
// @ts-expect-error The directly executed .mjs helper intentionally has no declaration file.
import { planTests, readsRepository, rustEmbeddedFiles, rustSuiteModules } from "../../scripts/test-plan.mjs";

const suiteModules = rustSuiteModules([
  {
    suite: "operations",
    source: '#[path = "../operations_tests.rs"]\nmod operations_tests;\n#[path = "../trash_tests.rs"]\r\nmod trash_tests;\n',
  },
  { suite: "media", source: '#[path = "../preview_tests.rs"]\nmod preview_tests;\n' },
]);
const repositoryReaders = ["tests/version.test.ts", "tests/config/tauri-csp.test.ts"];

function plan(changed: string[], platform = "darwin") {
  return planTests({ changed, full: false, platform, suiteModules, repositoryReaders });
}

describe("the default test plan", () => {
  it("runs nothing when nothing differs from HEAD", () => {
    expect(plan([])).toEqual({
      hidden: false,
      specs: false,
      typecheck: false,
      vitest: null,
      bundle: false,
      rust: null,
      heavy: false,
      windowsPackaging: false,
    });
  });

  it("runs only the hidden-character scan for documentation", () => {
    expect(plan(["README.md", "src-tauri/NOTES.md"])).toMatchObject({
      hidden: true,
      specs: false,
      typecheck: false,
      vitest: null,
      rust: null,
    });
  });

  it("typechecks and runs related and repository-reading tests for a TypeScript change, without Rust", () => {
    expect(plan(["src/utils/zoom.ts"])).toMatchObject({
      typecheck: true,
      vitest: ["src/utils/zoom.ts", ...repositoryReaders],
      rust: null,
    });
  });

  it("runs the related and repository-reading tests without the type check for a stylesheet change", () => {
    expect(plan(["src/App.css"])).toMatchObject({
      typecheck: false,
      vitest: ["src/App.css", ...repositoryReaders],
    });
    expect(plan(["specs/index.md"])).toMatchObject({
      specs: true,
      vitest: ["specs/index.md", ...repositoryReaders],
    });
  });

  it("typechecks for a JSON change, since modules import JSON", () => {
    expect(plan(["src/strings.json"])).toMatchObject({ typecheck: true, vitest: ["src/strings.json", ...repositoryReaders] });
  });

  it("typechecks when the TypeScript configuration or dependencies change", () => {
    expect(plan(["tsconfig.test.json"]).typecheck).toBe(true);
    expect(plan(["package-lock.json"]).typecheck).toBe(true);
  });

  it("narrows Rust to the suites and modules of changed root test modules", () => {
    expect(
      plan(["src-tauri/tests/trash_tests.rs", "src-tauri/tests/operations_tests.rs", "src-tauri/tests/preview_tests.rs"])
        .rust,
    ).toEqual({
      targets: ["operations_test_suite", "media_test_suite"],
      filters: ["trash_tests::", "operations_tests::", "preview_tests::"],
    });
  });

  it("never runs the heavy suite, whatever changes", () => {
    expect(plan(["src-tauri/tests/face_heavy_tests.rs", "src-tauri/src/face.rs"]).heavy).toBe(false);
  });

  it("runs every Rust test for any other Rust change", () => {
    expect(plan(["src-tauri/tests/trash_tests.rs", "src-tauri/src/trash.rs"]).rust).toBe("all");
    expect(plan(["src-tauri/tests/suites/operations.rs"]).rust).toBe("all");
    expect(plan(["src-tauri/Cargo.toml"])).toMatchObject({
      rust: "all",
      vitest: ["src-tauri/Cargo.toml", ...repositoryReaders],
    });
  });

  it("counts a file Rust embeds as a Rust change, wherever it lives", () => {
    const rustInputs = rustEmbeddedFiles([
      { file: "src-tauri/src/menu.rs", source: 'const EN: &str = include_str!("../../src/locales/en.json");' },
      {
        file: "src-tauri/src/icons.rs",
        source: 'static ICON: &[u8] = include_bytes!( "../icons/app.png" );\nconst BUILT: &str = include_str!(concat!(env!("OUT_DIR"), "/x"));',
      },
    ]);
    expect(rustInputs).toEqual(["src/locales/en.json", "src-tauri/icons/app.png"]);
    expect(
      planTests({ changed: ["src/locales/en.json"], full: false, platform: "darwin", suiteModules, repositoryReaders, rustInputs }),
    ).toMatchObject({ typecheck: true, rust: "all", vitest: ["src/locales/en.json", ...repositoryReaders] });
    expect(plan(["src/locales/en.json"]).rust).toBeNull();
  });

  it("runs the Windows packaging test only on Windows, when packaging changes", () => {
    expect(plan(["scripts/package.ps1"], "win32").windowsPackaging).toBe(true);
    expect(plan(["scripts/package.ps1"], "darwin").windowsPackaging).toBe(false);
    expect(plan(["src/utils/zoom.ts"], "win32").windowsPackaging).toBe(false);
  });
});

describe("the full run plan", () => {
  it("runs every lane regardless of changes", () => {
    const full = planTests({ changed: [], full: true, platform: "darwin", suiteModules, repositoryReaders });
    expect(full).toEqual({
      hidden: true,
      specs: true,
      typecheck: true,
      vitest: "all",
      bundle: true,
      rust: "all",
      heavy: true,
      windowsPackaging: false,
    });
    expect(
      planTests({ changed: [], full: true, platform: "win32", suiteModules, repositoryReaders })
        .windowsPackaging,
    ).toBe(true);
  });
});

describe("repository readers", () => {
  it("are the tests that read files through Node", () => {
    expect(readsRepository('import { readFileSync } from "node:fs";')).toBe(true);
    expect(readsRepository('import { readFile } from "node:fs/promises";')).toBe(true);
    expect(readsRepository('import { spawnSync } from "node:child_process";')).toBe(true);
    expect(readsRepository("import { readFileSync } from 'node:fs';")).toBe(true);
    expect(readsRepository('import { existsSync } from "fs";')).toBe(true);
    expect(readsRepository('const fs = await import("node:fs");')).toBe(true);
    expect(readsRepository('import path from "node:path";')).toBe(false);
    expect(readsRepository('import { fsync } from "./fsync";')).toBe(false);
  });
});
