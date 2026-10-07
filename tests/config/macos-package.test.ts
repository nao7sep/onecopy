import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";

const packageScript = readFileSync(path.resolve("scripts/package.sh"), "utf8");
const roots: string[] = [];
afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function packageFixture(dmgs = 1, apps = 1) {
  const root = mkdtempSync(path.join(tmpdir(), "onecopy-package-test-"));
  roots.push(root);
  for (const directory of ["scripts", "bin", "node_modules/.bin", "src-tauri/target/release/bundle/dmg", "src-tauri/target/release/bundle/macos/OldOneCopy.app"]) {
    mkdirSync(path.join(root, directory), { recursive: true });
  }
  writeFileSync(path.join(root, "scripts/package.sh"), packageScript);
  writeFileSync(path.join(root, "src-tauri/tauri.conf.json"), JSON.stringify({ version: "0.2.0" }));
  symlinkSync(process.execPath, path.join(root, "bin/node"));
  writeFileSync(path.join(root, "bin/ditto"), '#!/bin/sh\nfor arg; do output="$arg"; done\nprintf "portable" > "$output"\n', { mode: 0o755 });
  writeFileSync(path.join(root, "node_modules/.bin/tauri"), [
    "#!/bin/sh",
    "mkdir -p src-tauri/target/release/bundle/dmg src-tauri/target/release/bundle/macos",
    ...Array.from({ length: apps }, (_, index) => `mkdir -p src-tauri/target/release/bundle/macos/OneCopy${index}.app`),
    ...Array.from({ length: dmgs }, (_, index) => `printf "current-build" > src-tauri/target/release/bundle/dmg/OneCopy_0.2.0_${index}.dmg`),
  ].join("\n") + "\n", { mode: 0o755 });
  writeFileSync(path.join(root, "src-tauri/target/release/bundle/dmg/OneCopy_0.1.0_aarch64.dmg"), "stale-build");
  return { root, run: () => spawnSync("/bin/bash", [path.join(root, "scripts/package.sh")], {
    cwd: root,
    encoding: "utf8",
    timeout: 5_000,
    env: { ...process.env, PATH: `${path.join(root, "bin")}${path.delimiter}/usr/bin${path.delimiter}/bin` },
  }) };
}

describe.skipIf(process.platform !== "darwin")("macOS package artifact collection", () => {
  it("collects this build's DMG when an older version was left beside it", () => {
    const fixture = packageFixture();
    const result = fixture.run();
    expect(result.status, result.stderr).toBe(0);
    expect(readFileSync(path.join(fixture.root, "artifacts/OneCopy-0.2.0.dmg"), "utf8")).toBe("current-build");
  });

  it.each([
    [0, 1, "no DMG"],
    [1, 0, "no app"],
    [2, 1, "ambiguous DMGs"],
    [1, 2, "ambiguous apps"],
  ])("fails for %s DMGs/%s apps (%s) instead of selecting stale or ambiguous outputs", (dmgs, apps) => {
    const fixture = packageFixture(dmgs, apps);
    const result = fixture.run();
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("exactly one .dmg and one .app");
    expect(() => readFileSync(path.join(fixture.root, "artifacts/OneCopy-0.2.0.dmg"))).toThrow();
  });
});
