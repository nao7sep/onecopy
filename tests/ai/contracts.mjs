const SHA256 = /^[0-9a-f]{64}$/;
const TEST_IDS = new Set(["face", "audio-transcription", "video-transcription"]);
const REQUIREMENTS = new Set(["face-scoring", "transcription"]);

function object(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError(`${label} must be an object`);
  }
  return value;
}

function text(value, label) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new TypeError(`${label} must be a non-empty string`);
  }
  return value;
}

function integer(value, label, minimum = 0) {
  if (!Number.isSafeInteger(value) || value < minimum) {
    throw new TypeError(`${label} must be an integer >= ${minimum}`);
  }
  return value;
}

function finite(value, label, minimum = 0, { nullable = false } = {}) {
  if (nullable && value === null) return value;
  if (!Number.isFinite(value) || value < minimum) {
    throw new TypeError(`${label} must be a finite number >= ${minimum}${nullable ? " or null" : ""}`);
  }
  return value;
}

function timestamp(value, label) {
  text(value, label);
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test(value) ||
      new Date(value).toISOString() !== value) {
    throw new TypeError(`${label} must be a canonical UTC millisecond timestamp`);
  }
  return value;
}

function digest(value, label) {
  const selected = text(value, label);
  if (!SHA256.test(selected)) throw new TypeError(`${label} is invalid`);
  return selected;
}

function acceleration(testId, value, label, { nullable = false } = {}) {
  if (nullable && value === null) return value;
  const allowed = testId === "face" ? new Set(["none"]) : new Set(["none", "metal"]);
  if (!allowed.has(value)) throw new TypeError(`${label} is invalid`);
  return value;
}

function failure(value, label) {
  const input = object(value, label);
  text(input.category, `${label}.category`);
  text(input.message, `${label}.message`);
  return input;
}

function validateBuildFacts(value, label) {
  const build = object(value, label);
  if (!new Set(["darwin", "win32"]).has(build.platform)) {
    throw new TypeError(`${label}.platform is invalid`);
  }
  text(build.architecture, `${label}.architecture`);
  text(build.targetTriple, `${label}.targetTriple`);
  const toolchain = object(build.toolchain, `${label}.toolchain`);
  for (const name of ["rustc", "cargo", "node"]) text(toolchain[name], `${label}.toolchain.${name}`);
  if (!Array.isArray(build.compileFeatures) || build.compileFeatures.length === 0) {
    throw new TypeError(`${label}.compileFeatures must be a non-empty array`);
  }
  const features = new Set();
  for (const feature of build.compileFeatures) {
    text(feature, `${label}.compileFeatures entry`);
    if (features.has(feature)) throw new TypeError(`${label}.compileFeatures must be unique`);
    features.add(feature);
  }
  if (!Array.isArray(build.capabilities) || build.capabilities.length === 0) {
    throw new TypeError(`${label}.capabilities must be a non-empty array`);
  }
  const capabilityIds = new Set();
  for (const capability of build.capabilities) {
    object(capability, `${label}.capability`);
    const feature = text(capability.feature, `${label}.capability.feature`);
    if (capabilityIds.has(feature)) throw new TypeError(`${label}.capabilities must be unique`);
    capabilityIds.add(feature);
    if (!Array.isArray(capability.options) || capability.options.length === 0) {
      throw new TypeError(`${label}.capability.options must be a non-empty array`);
    }
    const options = new Set();
    for (const option of capability.options) {
      text(option, `${label}.capability.option`);
      if (options.has(option)) throw new TypeError(`${label}.capability.options must be unique`);
      options.add(option);
    }
  }
  return build;
}

export function validateFixtureReference(value, label = "fixture") {
  const input = object(value, label);
  const basename = text(input.basename, `${label}.basename`);
  if (basename !== basename.split(/[\\/]/).at(-1) || basename === "." || basename === "..") {
    throw new TypeError(`${label}.basename must not contain a path`);
  }
  const sha256 = text(input.sha256, `${label}.sha256`);
  if (!SHA256.test(sha256)) {
    throw new TypeError(`${label}.sha256 must be 64 lowercase hexadecimal characters`);
  }
  integer(input.bytes, `${label}.bytes`, 1);
  return { basename, sha256, bytes: input.bytes };
}

export function normalizeAcceleration(testId, value) {
  const selected = value ?? "none";
  const allowed = testId === "face" ? new Set(["none"]) : new Set(["none", "metal"]);
  if (!allowed.has(selected)) {
    throw new TypeError(`${testId} does not support acceleration ${String(selected)}`);
  }
  if (selected === "metal" && !(process.platform === "darwin" && process.arch === "arm64")) {
    throw new TypeError("metal transcription requires Apple-silicon macOS");
  }
  return selected;
}

export function requirementFor(testId) {
  if (testId === "face") return "face-scoring";
  if (testId === "audio-transcription" || testId === "video-transcription") {
    return "transcription";
  }
  throw new TypeError(`unknown AI test id ${String(testId)}`);
}

export function validateParameters(value) {
  const input = object(value, "parameters");
  if (input.schemaVersion !== 2) {
    throw new TypeError(`unsupported parameter schema version ${String(input.schemaVersion)}`);
  }
  text(input.profileId, "profileId");
  integer(input.profileVersion, "profileVersion", 1);
  if (!Array.isArray(input.cases) || input.cases.length === 0) {
    throw new TypeError("cases must be a non-empty array");
  }
  const ids = new Set();
  const cases = input.cases.map((candidate, index) => {
    const item = object(candidate, `cases[${index}]`);
    const id = text(item.id, `cases[${index}].id`);
    if (!TEST_IDS.has(id) || ids.has(id)) {
      throw new TypeError(`cases[${index}].id must be unique and supported`);
    }
    ids.add(id);
    const fixtures = item.fixtures?.map((fixture, fixtureIndex) =>
      validateFixtureReference(fixture, `cases[${index}].fixtures[${fixtureIndex}]`),
    );
    if (!fixtures?.length) {
      throw new TypeError(`cases[${index}].fixtures must be a non-empty array`);
    }
    const timeoutMs = integer(item.timeoutMs, `cases[${index}].timeoutMs`, 1_000);
    const acceleration = normalizeAcceleration(id, item.acceleration);
    return {
      id,
      requirement: requirementFor(id),
      fixtures,
      timeoutMs,
      acceleration,
    };
  });
  return {
    schemaVersion: 2,
    profileId: input.profileId,
    profileVersion: input.profileVersion,
    cases,
  };
}

export function validateResult(value) {
  const result = object(value, "result");
  if (result.schemaVersion !== 2 && result.schemaVersion !== 3) {
    throw new TypeError(`unsupported result schema version ${String(result.schemaVersion)}`);
  }
  const legacy = result.schemaVersion === 2;
  if (!["running", "passed", "failed", "interrupted"].includes(result.outcome)) {
    throw new TypeError("result.outcome is invalid");
  }
  text(result.profileId, "result.profileId");
  integer(result.profileVersion, "result.profileVersion", 1);
  if (!new Set(["live", "benchmark"]).has(result.mode)) {
    throw new TypeError("result.mode is invalid");
  }
  timestamp(result.startedAtUtc, "result.startedAtUtc");
  if (result.outcome === "running") {
    if ("finishedAtUtc" in result) throw new TypeError("running result must not have finishedAtUtc");
  } else {
    timestamp(result.finishedAtUtc, "result.finishedAtUtc");
  }
  if ("failure" in result) failure(result.failure, "result.failure");
  if (["running", "passed"].includes(result.outcome) && "failure" in result) {
    throw new TypeError(`${result.outcome} result must not have failure`);
  }
  const executable = object(result.executable, "result.executable");
  const executableBasename = text(executable.basename, "result.executable.basename");
  if (executableBasename !== executableBasename.split(/[\\/]/).at(-1)) {
    throw new TypeError("result.executable.basename must not contain a path");
  }
  digest(executable.sha256, "result.executable.sha256");
  digest(result.buildManifestSha256, "result.buildManifestSha256");
  if (!legacy) validateBuildFacts(result.build, "result.build");
  const machine = object(result.machine, "result.machine");
  if (!new Set(["darwin", "win32"]).has(machine.platform)) {
    throw new TypeError("result.machine.platform is invalid");
  }
  text(machine.osVersion, "result.machine.osVersion");
  text(machine.architecture, "result.machine.architecture");
  text(machine.cpuModel, "result.machine.cpuModel");
  integer(machine.logicalCpuCount, "result.machine.logicalCpuCount", 1);
  integer(machine.totalMemoryBytes, "result.machine.totalMemoryBytes", 1);
  const source = object(result.source, "result.source");
  if (!/^[0-9a-f]{40}$/.test(text(source.commit, "result.source.commit"))) {
    throw new TypeError("result.source.commit is invalid");
  }
  if (typeof source.dirty !== "boolean") throw new TypeError("result.source.dirty must be boolean");
  digest(source.trackedDiffSha256, "result.source.trackedDiffSha256");
  integer(source.untrackedCount, "result.source.untrackedCount");
  digest(source.untrackedContentSha256, "result.source.untrackedContentSha256");
  if (!Array.isArray(result.cases)) throw new TypeError("result.cases must be an array");
  const scenarioIds = new Set();
  for (const [index, item] of result.cases.entries()) {
    object(item, `result.cases[${index}]`);
    if (!TEST_IDS.has(item.scenarioId) || scenarioIds.has(item.scenarioId)) {
      throw new TypeError(`result.cases[${index}].scenarioId is invalid`);
    }
    scenarioIds.add(item.scenarioId);
    if (!["running", "passed", "failed", "interrupted"].includes(item.outcome)) {
      throw new TypeError(`result.cases[${index}].outcome is invalid`);
    }
    timestamp(item.startedAtUtc, `result.cases[${index}].startedAtUtc`);
    if (!legacy) integer(item.timeoutMs, `result.cases[${index}].timeoutMs`, 1_000);
    if (item.outcome === "running") {
      if ("finishedAtUtc" in item) {
        throw new TypeError(`result.cases[${index}] running case must not have finishedAtUtc`);
      }
    } else {
      timestamp(item.finishedAtUtc, `result.cases[${index}].finishedAtUtc`);
    }
    if (!Array.isArray(item.dependencies) || item.dependencies.length === 0) {
      throw new TypeError(`result.cases[${index}].dependencies must be non-empty`);
    }
    for (const [dependencyIndex, dependency] of item.dependencies.entries()) {
      const label = `result.cases[${index}].dependencies[${dependencyIndex}]`;
      object(dependency, label);
      text(dependency.id, `${label}.id`);
      digest(dependency.sha256, `${label}.sha256`);
      integer(dependency.bytes, `${label}.bytes`, 1);
      if (dependency.version !== null) text(dependency.version, `${label}.version`);
    }
    if (!Array.isArray(item.fixtures) || item.fixtures.length === 0) {
      throw new TypeError(`result.cases[${index}].fixtures must be non-empty`);
    }
    item.fixtures.forEach((fixture, fixtureIndex) =>
      validateFixtureReference(fixture, `result.cases[${index}].fixtures[${fixtureIndex}]`));
    const configuredAcceleration = acceleration(item.scenarioId, item.configuredAcceleration,
      `result.cases[${index}].configuredAcceleration`);
    const observedAcceleration = acceleration(item.scenarioId, item.observedAcceleration,
      `result.cases[${index}].observedAcceleration`, { nullable: true });
    if (!legacy && observedAcceleration !== null && observedAcceleration !== configuredAcceleration) {
      throw new TypeError(`result.cases[${index}].observedAcceleration differs from configuredAcceleration`);
    }
    if (item.outcome === "passed") {
      object(item.correctness, `result.cases[${index}].correctness`);
      if ("failure" in item) throw new TypeError(`result.cases[${index}] passed case has failure`);
    } else if (item.outcome !== "running") {
      failure(item.failure, `result.cases[${index}].failure`);
    }
    if (result.mode === "live") {
      if (item.observations !== null) {
        throw new TypeError(`result.cases[${index}].observations must be null in live mode`);
      }
    } else if (item.outcome === "running") {
      if (item.observations !== null) {
        throw new TypeError(`result.cases[${index}] running observations must be null`);
      }
    } else if (item.outcome === "passed" || item.observations !== null) {
      const observations = object(item.observations, `result.cases[${index}].observations`);
      finite(observations.wallMs, `result.cases[${index}].observations.wallMs`, Number.MIN_VALUE);
      finite(observations.peakProcessTreeBytes,
        `result.cases[${index}].observations.peakProcessTreeBytes`, 0, { nullable: true });
      if (!Array.isArray(observations.phases)) {
        throw new TypeError(`result.cases[${index}].observations.phases must be an array`);
      }
      for (const [phaseIndex, phase] of observations.phases.entries()) {
        const label = `result.cases[${index}].observations.phases[${phaseIndex}]`;
        object(phase, label);
        text(phase.phase, `${label}.phase`);
        finite(phase.wallMs, `${label}.wallMs`);
      }
    }
  }
  if (result.outcome !== "running" && result.cases.some(({ outcome }) => outcome === "running")) {
    throw new TypeError("terminal result contains a running case");
  }
  if (result.outcome === "passed" &&
      (result.cases.length === 0 || result.cases.some(({ outcome }) => outcome !== "passed"))) {
    throw new TypeError("passed result must contain only passed cases");
  }
  if (result.outcome === "failed" && !result.cases.some(({ outcome }) => outcome === "failed")) {
    throw new TypeError("failed result must contain a failed case");
  }
  if (result.outcome === "interrupted" &&
      !result.cases.some(({ outcome }) => outcome === "interrupted") && !("failure" in result)) {
    throw new TypeError("interrupted result must contain an interrupted case or runner failure");
  }
  return result;
}

export function requirementsFor(parameters) {
  return [...new Set(validateParameters(parameters).cases.map(({ requirement }) => requirement))];
}

export function validatePreparedContext(value) {
  const context = object(value, "prepared context");
  if (!Array.isArray(context.requirements) || context.requirements.length === 0) {
    throw new TypeError("prepared context requirements must be a non-empty array");
  }
  const requirements = new Set();
  for (const requirement of context.requirements) {
    if (!REQUIREMENTS.has(requirement) || requirements.has(requirement)) {
      throw new TypeError("prepared context requirements must be unique and supported");
    }
    requirements.add(requirement);
  }
  if (!Array.isArray(context.artifacts) || context.artifacts.length === 0) {
    throw new TypeError("prepared context artifacts must be a non-empty array");
  }
  const artifactIds = new Set();
  for (const artifact of context.artifacts) {
    object(artifact, "prepared artifact");
    const id = text(artifact.id, "prepared artifact id");
    if (artifactIds.has(id)) throw new TypeError(`prepared artifact is duplicated: ${id}`);
    artifactIds.add(id);
    if (!["binary", "runtime", "model"].includes(artifact.kind)) {
      throw new TypeError(`prepared artifact ${id} has an invalid kind`);
    }
    if (!Array.isArray(artifact.requirements) || artifact.requirements.length === 0 ||
        artifact.requirements.some((requirement) => !requirements.has(requirement))) {
      throw new TypeError(`prepared artifact ${id} has invalid requirements`);
    }
    if (artifact.readiness !== "current") {
      throw new TypeError(`prepared artifact ${id} is not current`);
    }
    const identity = object(artifact.identity, `prepared artifact ${id} identity`);
    if (!SHA256.test(identity.sha256)) {
      throw new TypeError(`prepared artifact ${id} digest is invalid`);
    }
    integer(identity.bytes, `prepared artifact ${id} bytes`, 1);
    if (identity.version !== null) text(identity.version, `prepared artifact ${id} version`);
  }
  for (const requirement of requirements) {
    if (!context.artifacts.some((artifact) => artifact.requirements.includes(requirement))) {
      throw new TypeError(`prepared context has no artifact for ${requirement}`);
    }
  }
  if (!Array.isArray(context.capabilities)) {
    throw new TypeError("prepared context capabilities must be an array");
  }
  for (const capability of context.capabilities) {
    text(capability.feature, "prepared capability feature");
    if (!Array.isArray(capability.options) || capability.options.length === 0) {
      throw new TypeError("prepared capability options must be a non-empty array");
    }
    capability.options.forEach((option) => text(option.id, "prepared capability option"));
  }
  return context;
}

export function validateBuildManifest(value, parameters) {
  const manifest = object(value, "build manifest");
  if (manifest.schemaVersion !== 2) throw new TypeError("unsupported build manifest schema version");
  if (!manifest.preparer || !SHA256.test(manifest.preparer.sha256)) {
    throw new TypeError("build manifest preparer digest is invalid");
  }
  if (!manifest.scenarioExecutable || !SHA256.test(manifest.scenarioExecutable.sha256)) {
    throw new TypeError("build manifest scenario executable digest is invalid");
  }
  for (const [label, artifact] of [
    ["preparer", manifest.preparer],
    ["scenario executable", manifest.scenarioExecutable],
  ]) {
    const name = text(artifact.basename, `build manifest ${label} basename`);
    if (name !== name.split(/[\\/]/).at(-1)) {
      throw new TypeError(`build manifest ${label} basename must not contain a path`);
    }
  }
  const build = validateBuildFacts({
    platform: manifest.platform,
    architecture: manifest.architecture,
    targetTriple: manifest.targetTriple,
    toolchain: manifest.toolchain,
    compileFeatures: manifest.compileFeatures,
    capabilities: manifest.preparedContext?.capabilities?.map(({ feature, options }) => ({
      feature,
      options: options?.map(({ id }) => id),
    })),
  }, "build manifest");
  if (!build.compileFeatures.includes("ai-test-support")) {
    throw new TypeError("build manifest does not contain AI test support");
  }
  const prepared = validatePreparedContext(manifest.preparedContext);
  const capabilities = new Map(prepared.capabilities.map(({ feature, options }) => [
    feature,
    new Set(options.map(({ id }) => id)),
  ]));
  for (const item of validateParameters(parameters).cases) {
    const feature = item.id === "face" ? "face-scoring" : "transcription";
    if (!capabilities.get(feature)?.has(item.acceleration)) {
      throw new TypeError(`prepared binary does not offer ${item.acceleration} for ${feature}`);
    }
  }
  const present = new Set(prepared.requirements);
  for (const requirement of requirementsFor(parameters)) {
    if (!present.has(requirement)) {
      throw new TypeError(`build manifest is missing requirement ${requirement}`);
    }
  }
  return manifest;
}
