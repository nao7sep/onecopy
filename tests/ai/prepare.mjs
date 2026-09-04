import { execFileSync } from "node:child_process";
import {
  constants,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
} from "node:fs";
import { arch, platform } from "node:os";
import { basename, resolve } from "node:path";
import { cloneArtifactTree } from "./artifact-tree.mjs";
import { requirementsFor, validateParameters } from "./contracts.mjs";
import { indexFixtureRoot, resolveFixtures, sha256File } from "./fixtures.mjs";
import { runOwned } from "./process.mjs";
import { writeAtomicReport } from "./report.mjs";
import { sourceState } from "./source-state.mjs";

const executable = (name) => (process.platform === "win32" ? `${name}.exe` : name);

export async function runPreparationStep(command, args, options = {}) {
  const result = await runOwned(command, args, {
    cwd: options.cwd,
    timeoutMs: options.timeout ?? 4 * 60 * 60 * 1_000,
    measureMemory: false,
    signal: options.signal,
    onStdout: options.capture ? undefined : (chunk) => process.stdout.write(chunk),
    onStderr: options.capture ? undefined : (chunk) => process.stderr.write(chunk),
  });
  if (result.interrupted) {
    throw new Error(`${basename(command)} was interrupted during preparation`);
  }
  if (result.timedOut) {
    throw new Error(`${basename(command)} timed out during preparation`);
  }
  if (result.code !== 0) {
    throw new Error(`${basename(command)} failed during preparation${result.stderr ? `: ${result.stderr.trim()}` : ""}`);
  }
  return result.stdout.trim();
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

export function publishVersionedBinary(source, outputBin, stem) {
  mkdirSync(outputBin, { recursive: true });
  const sha256 = sha256File(source);
  const extension = process.platform === "win32" ? ".exe" : "";
  const publishedBasename = `${stem}-${sha256.slice(0, 16)}${extension}`;
  const published = resolve(outputBin, publishedBasename);
  if (existsSync(published)) {
    if (sha256File(published) !== sha256) {
      throw new Error(`prepared ${stem} digest-named file is corrupt`);
    }
    return { basename: publishedBasename, sha256 };
  }
  const partial = resolve(outputBin, `.${publishedBasename}.${process.pid}.partial`);
  rmSync(partial, { force: true });
  try {
    copyFileSync(source, partial, constants.COPYFILE_EXCL);
    if (sha256File(partial) !== sha256) {
      throw new Error(`prepared ${stem} copy digest mismatch`);
    }
    renameSync(partial, published);
  } finally {
    rmSync(partial, { force: true });
  }
  return { basename: publishedBasename, sha256 };
}

export async function prepare({
  repositoryRoot,
  parameterPath,
  fixtureRoot,
  preparedRoot,
  reuseManagedRoot,
  signal,
}) {
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

  await runPreparationStep("cargo", [
    "build",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "--features",
    "ai-test-support",
    "--bin",
    "onecopy-ai-preparer",
    "--bin",
    "onecopy-ai-scenario",
  ], { cwd: repositoryRoot, signal });

  const preparer = resolve(repositoryRoot, "src-tauri", "target", "debug", executable("onecopy-ai-preparer"));
  const requirements = requirementsFor(parameters);
  const prepareOutput = await runPreparationStep(preparer, ["prepare", artifactHome, ...requirements], {
    cwd: repositoryRoot,
    capture: true,
    signal,
  });
  const preparedContext = prepareOutput
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => JSON.parse(line))
    .findLast((event) => event.event === "prepared-context")?.context;
  if (!preparedContext) throw new Error("managed dependency preparation did not return a context");

  const scenario = resolve(repositoryRoot, "src-tauri", "target", "debug", executable("onecopy-ai-scenario"));
  const publishedPreparer = publishVersionedBinary(preparer, outputBin, "onecopy-ai-preparer");
  const scenarioExecutable = publishVersionedBinary(scenario, outputBin, "onecopy-ai-scenario");
  const manifest = {
    schemaVersion: 2,
    source: sourceState(repositoryRoot),
    platform: platform(),
    architecture: arch(),
    targetTriple: rustTargetTriple(),
    toolchain: {
      rustc: version("rustc", ["--version"]),
      cargo: version("cargo", ["--version"]),
      node: process.version,
    },
    compileFeatures: ["ai-test-support"],
    preparer: publishedPreparer,
    scenarioExecutable,
    preparedContext,
  };
  const manifestPath = resolve(outputBin, "onecopy.ai-build.json");
  writeAtomicReport(manifestPath, manifest);
  return { manifestPath, artifactHome, parameters };
}
