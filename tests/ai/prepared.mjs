import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { dependenciesFor, validateBuildManifest } from "./contracts.mjs";
import { sha256File } from "./fixtures.mjs";
import { sourceState } from "./source-state.mjs";

const executable = (name) => (process.platform === "win32" ? `${name}.exe` : name);

function runJsonLines(command, args, repositoryRoot) {
  const result = spawnSync(command, args, {
    cwd: repositoryRoot,
    encoding: "utf8",
    env: { ...process.env, ONECOPY_AI_OFFLINE: "1" },
    timeout: 4 * 60 * 60 * 1_000,
    windowsHide: true,
  });
  if (result.status !== 0) {
    throw new Error(result.stderr?.trim() || "prepared-artifact verification failed");
  }
  return result.stdout.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
}

function dependencyIdentity(artifact) {
  return {
    id: artifact.id,
    sha256: artifact.sha256,
    bytes: artifact.bytes,
    version: artifact.version ?? null,
  };
}

export function loadPrepared(repositoryRoot, preparedRoot, parameters) {
  const manifest = JSON.parse(
    readFileSync(resolve(preparedRoot, "bin", "onecopy.ai-build.json"), "utf8"),
  );
  validateBuildManifest(manifest, parameters);
  const binary = resolve(preparedRoot, "bin", executable("onecopy"));
  const driver = resolve(preparedRoot, "bin", executable("onecopy-ai-preparer"));
  if (sha256File(binary) !== manifest.binary.sha256) {
    throw new Error("prepared application digest mismatch");
  }
  if (sha256File(driver) !== manifest.driver.sha256) {
    throw new Error("prepared test-driver digest mismatch");
  }
  if (JSON.stringify(sourceState(repositoryRoot)) !== JSON.stringify(manifest.source)) {
    throw new Error("prepared application does not match the current source state");
  }

  const dependencyIds = dependenciesFor(parameters);
  const ready = runJsonLines(
    driver,
    ["verify", resolve(preparedRoot, "managed"), ...dependencyIds],
    repositoryRoot,
  ).findLast((event) => event.event === "ready");
  if (!ready) throw new Error("prepared dependency verification emitted no readiness result");
  const expected = manifest.dependencies
    .filter(({ id }) => dependencyIds.includes(id))
    .map(dependencyIdentity)
    .sort((left, right) => left.id.localeCompare(right.id));
  const actual = ready.artifacts
    .map(dependencyIdentity)
    .sort((left, right) => left.id.localeCompare(right.id));
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error("prepared dependency bytes differ from the build manifest");
  }

  const capabilities = runJsonLines(driver, ["capabilities"], repositoryRoot).at(-1);
  const actualCapabilities = capabilities.map(({ feature, options }) => ({
    feature,
    modes: options.map(({ id }) => id),
  }));
  if (JSON.stringify(actualCapabilities) !== JSON.stringify(manifest.accelerationCapabilities)) {
    throw new Error("prepared test-driver capabilities differ from the build manifest");
  }
  return { manifest, binary, driver };
}
