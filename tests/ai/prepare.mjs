import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { arch, platform } from "node:os";
import { basename, resolve } from "node:path";
import { cloneArtifactTree } from "./artifact-tree.mjs";
import { dependenciesFor, validateParameters } from "./contracts.mjs";
import { indexFixtureRoot, resolveFixtures, sha256File } from "./fixtures.mjs";
import { assertPrivacySafe } from "./report.mjs";
import { sourceState } from "./source-state.mjs";

const executable = (name) => (process.platform === "win32" ? `${name}.exe` : name);

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd,
    encoding: "utf8",
    stdio: options.capture ? "pipe" : "inherit",
    timeout: options.timeout ?? 4 * 60 * 60 * 1_000,
    windowsHide: true,
  });
  if (result.status !== 0) {
    throw new Error(`${basename(command)} failed during preparation${result.stderr ? `: ${result.stderr.trim()}` : ""}`);
  }
  return result.stdout?.trim() ?? "";
}

function version(command, args) {
  return execFileSync(command, args, { encoding: "utf8", timeout: 30_000, windowsHide: true })
    .trim()
    .split(/\r?\n/)[0];
}

function rustTargetTriple() {
  const details = execFileSync("rustc", ["-vV"], {
    encoding: "utf8",
    timeout: 30_000,
    windowsHide: true,
  });
  const host = details.split(/\r?\n/).find((line) => line.startsWith("host: "))?.slice(6);
  if (!host) throw new Error("rustc did not report its host target triple");
  return host;
}

export function prepare({ repositoryRoot, parameterPath, fixtureRoot, preparedRoot, reuseManagedRoot }) {
  const parameters = validateParameters(JSON.parse(readFileSync(parameterPath, "utf8")));
  const references = parameters.cases.flatMap((item) => item.fixtures);
  resolveFixtures(indexFixtureRoot(fixtureRoot), references);

  const artifactHome = resolve(preparedRoot, "managed");
  const outputBin = resolve(preparedRoot, "bin");
  mkdirSync(artifactHome, { recursive: true });
  mkdirSync(outputBin, { recursive: true });
  if (reuseManagedRoot) {
    for (const name of ["bin", "models"]) {
      const source = resolve(reuseManagedRoot, name);
      if (existsSync(source)) cloneArtifactTree(source, resolve(artifactHome, name));
    }
    const facts = resolve(reuseManagedRoot, "dependencies.json");
    if (existsSync(facts)) copyFileSync(facts, resolve(artifactHome, "dependencies.json"));
  }

  run("cargo", [
    "build",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "--features",
    "app-e2e",
    "--bin",
    "onecopy-ai-preparer",
  ], { cwd: repositoryRoot });

  const preparer = resolve(repositoryRoot, "src-tauri", "target", "debug", executable("onecopy-ai-preparer"));
  const dependencyIds = dependenciesFor(parameters);
  const prepareOutput = run(preparer, ["prepare", artifactHome, ...dependencyIds], {
    cwd: repositoryRoot,
    capture: true,
  });
  const ready = prepareOutput
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => JSON.parse(line))
    .findLast((event) => event.event === "ready");
  if (!ready) throw new Error("managed dependency preparation did not report readiness");

  const npmCli = process.env.npm_execpath;
  if (!npmCli) throw new Error("npm executable path is unavailable");
  run(process.execPath, [npmCli, "run", "build:e2e"], { cwd: repositoryRoot });
  const builtApp = resolve(repositoryRoot, "src-tauri", "target", "debug", executable("onecopy"));
  const preparedApp = resolve(outputBin, executable("onecopy"));
  const preparedDriver = resolve(outputBin, executable("onecopy-ai-preparer"));
  copyFileSync(builtApp, preparedApp);
  copyFileSync(preparer, preparedDriver);

  const capabilities = JSON.parse(run(preparer, ["capabilities"], {
    cwd: repositoryRoot,
    capture: true,
  }));
  const manifest = {
    schemaVersion: 1,
    source: sourceState(repositoryRoot),
    platform: platform(),
    architecture: arch(),
    targetTriple: rustTargetTriple(),
    toolchain: {
      rustc: version("rustc", ["--version"]),
      cargo: version("cargo", ["--version"]),
      node: process.version,
    },
    compileFeatures: ["app-e2e"],
    accelerationCapabilities: capabilities.map(({ feature, options }) => ({
      feature,
      modes: options.map(({ id }) => id),
    })),
    binary: { basename: basename(preparedApp), sha256: sha256File(preparedApp) },
    driver: { basename: basename(preparedDriver), sha256: sha256File(preparedDriver) },
    dependencies: ready.artifacts.map(({ id, sha256, bytes, version: artifactVersion }) => ({
      id,
      sha256,
      bytes,
      version: artifactVersion,
    })),
  };
  assertPrivacySafe(manifest);
  const manifestPath = resolve(outputBin, "onecopy.ai-build.json");
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  return { manifestPath, artifactHome, parameters };
}
