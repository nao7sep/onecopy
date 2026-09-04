import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { basename, resolve } from "node:path";
import { requirementsFor, validateBuildManifest, validatePreparedContext } from "./contracts.mjs";
import { sha256File } from "./fixtures.mjs";
import { sourceState } from "./source-state.mjs";

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

export function dependenciesForCase(context, item) {
  return context.artifacts
    .filter(({ requirements }) => requirements.includes(item.requirement))
    .map(({ id, identity }) => ({
      id,
      sha256: identity.sha256,
      bytes: identity.bytes,
      version: identity.version ?? null,
    }));
}

export function loadPrepared(repositoryRoot, preparedRoot, parameters) {
  const manifest = JSON.parse(
    readFileSync(resolve(preparedRoot, "bin", "onecopy.ai-build.json"), "utf8"),
  );
  validateBuildManifest(manifest, parameters);
  const binary = resolve(preparedRoot, "bin", manifest.binary.basename);
  const driver = resolve(preparedRoot, "bin", manifest.driver.basename);
  if (basename(binary) !== manifest.binary.basename || basename(driver) !== manifest.driver.basename) {
    throw new Error("prepared manifest binary names must not contain paths");
  }
  if (sha256File(binary) !== manifest.binary.sha256) {
    throw new Error("prepared application digest mismatch");
  }
  if (sha256File(driver) !== manifest.driver.sha256) {
    throw new Error("prepared test-driver digest mismatch");
  }
  if (JSON.stringify(sourceState(repositoryRoot)) !== JSON.stringify(manifest.source)) {
    throw new Error("prepared application does not match the current source state");
  }

  const managedRoot = resolve(preparedRoot, "managed");
  const requirements = requirementsFor(parameters);
  const preparedContext = runJsonLines(
    driver,
    ["verify", managedRoot, ...requirements],
    repositoryRoot,
  ).findLast((event) => event.event === "prepared-context")?.context;
  if (!preparedContext) {
    throw new Error("prepared dependency verification emitted no prepared context");
  }
  validatePreparedContext(preparedContext);
  if (JSON.stringify(preparedContext) !== JSON.stringify(manifest.preparedContext)) {
    throw new Error("prepared context differs from the build manifest");
  }
  return Object.freeze({ manifest, binary, driver, managedRoot, preparedContext });
}
