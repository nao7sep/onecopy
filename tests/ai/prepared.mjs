import { spawnSync } from "node:child_process";
import { lstatSync, readFileSync, realpathSync } from "node:fs";
import { basename, isAbsolute, relative, resolve, sep } from "node:path";
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

function fingerprint(path) {
  const stat = lstatSync(path, { bigint: true });
  if (!stat.isFile()) throw new Error("prepared execution guard requires regular files");
  return [stat.dev, stat.ino, stat.mode, stat.size, stat.mtimeNs, stat.ctimeNs]
    .map((value) => value.toString());
}

function guardedPath(root, relativePath) {
  if (typeof relativePath !== "string" || relativePath.trim() === "" || isAbsolute(relativePath)) {
    throw new Error("prepared execution guard contains an invalid relative path");
  }
  const path = resolve(root, relativePath);
  const fromRoot = relative(root, path);
  if (fromRoot === "" || fromRoot === ".." || fromRoot.startsWith(`..${sep}`) || isAbsolute(fromRoot)) {
    throw new Error("prepared execution guard escapes its prepared root");
  }
  let component = root;
  for (const name of fromRoot.split(sep)) {
    component = resolve(component, name);
    if (lstatSync(component).isSymbolicLink()) {
      throw new Error("prepared execution guard refuses symbolic links");
    }
  }
  return path;
}

export function snapshotPreparedGuard(managedRoot, scenarioExecutable, artifacts) {
  if (!Array.isArray(artifacts) || artifacts.length === 0) {
    throw new Error("prepared verification emitted no artifact paths");
  }
  const seen = new Set();
  const files = [{ id: "scenario-executable", path: scenarioExecutable }];
  for (const artifact of artifacts) {
    if (!artifact || typeof artifact.id !== "string" || artifact.id.trim() === "" ||
        seen.has(artifact.id)) {
      throw new Error("prepared verification emitted invalid artifact paths");
    }
    seen.add(artifact.id);
    files.push({ id: artifact.id, path: guardedPath(managedRoot, artifact.relativePath) });
  }
  return Object.freeze(files.map(({ id, path }) => Object.freeze({
    id,
    path,
    fingerprint: fingerprint(path),
  })));
}

export function assertPreparedUnchanged(guard) {
  for (const file of guard) {
    let current;
    try {
      current = fingerprint(file.path);
    } catch {
      throw new Error("prepared files changed after complete preflight");
    }
    if (current.length !== file.fingerprint.length ||
        current.some((value, index) => value !== file.fingerprint[index])) {
      throw new Error("prepared files changed after complete preflight");
    }
  }
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
  const manifestPath = resolve(preparedRoot, "bin", "onecopy.ai-build.json");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  validateBuildManifest(manifest, parameters);
  const preparerPath = resolve(preparedRoot, "bin", manifest.preparer.basename);
  const scenarioExecutablePath = resolve(preparedRoot, "bin", manifest.scenarioExecutable.basename);
  if (basename(preparerPath) !== manifest.preparer.basename ||
      basename(scenarioExecutablePath) !== manifest.scenarioExecutable.basename) {
    throw new Error("prepared manifest executable names must not contain paths");
  }
  if (lstatSync(preparerPath).isSymbolicLink() || lstatSync(scenarioExecutablePath).isSymbolicLink()) {
    throw new Error("prepared executables must not be symbolic links");
  }
  if (sha256File(preparerPath) !== manifest.preparer.sha256) {
    throw new Error("prepared preparer digest mismatch");
  }
  if (sha256File(scenarioExecutablePath) !== manifest.scenarioExecutable.sha256) {
    throw new Error("prepared scenario executable digest mismatch");
  }
  if (JSON.stringify(sourceState(repositoryRoot)) !== JSON.stringify(manifest.source)) {
    throw new Error("prepared application does not match the current source state");
  }

  const preparer = realpathSync(preparerPath);
  const scenarioExecutable = realpathSync(scenarioExecutablePath);
  const managedRootForVerification = resolve(preparedRoot, "managed");
  const requirements = requirementsFor(parameters);
  const verificationEvents = runJsonLines(
    preparer,
    ["verify", managedRootForVerification, ...requirements],
    repositoryRoot,
  );
  const preparedContext = verificationEvents
    .findLast((event) => event.event === "prepared-context")?.context;
  if (!preparedContext) {
    throw new Error("prepared dependency verification emitted no prepared context");
  }
  validatePreparedContext(preparedContext);
  if (JSON.stringify(preparedContext) !== JSON.stringify(manifest.preparedContext)) {
    throw new Error("prepared context differs from the build manifest");
  }
  const managedRoot = realpathSync(managedRootForVerification);
  const artifactPaths = verificationEvents
    .findLast((event) => event.event === "prepared-artifact-paths")?.artifacts;
  if (!Array.isArray(artifactPaths) || artifactPaths.length !== preparedContext.artifacts.length ||
      artifactPaths.some((item) => !item || typeof item.id !== "string" ||
        !preparedContext.artifacts.some((artifact) => artifact.id === item.id))) {
    throw new Error("prepared artifact paths differ from the verified context");
  }
  const preparedGuard = snapshotPreparedGuard(managedRoot, scenarioExecutable, artifactPaths);
  return Object.freeze({
    manifest,
    buildManifestSha256: sha256File(manifestPath),
    preparer,
    scenarioExecutable,
    managedRoot,
    preparedContext,
    preparedGuard,
  });
}
