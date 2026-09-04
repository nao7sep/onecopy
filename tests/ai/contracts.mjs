const SHA256 = /^[0-9a-f]{64}$/;
const TEST_IDS = new Set(["face", "audio-transcription", "video-transcription"]);
const SURFACES = new Set(["adapter", "app"]);
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
  if (input.schemaVersion !== 1) {
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
    if (!SURFACES.has(item.surface)) {
      throw new TypeError(`cases[${index}].surface must be adapter or app`);
    }
    const fixtures = item.fixtures?.map((fixture, fixtureIndex) =>
      validateFixtureReference(fixture, `cases[${index}].fixtures[${fixtureIndex}]`),
    );
    if (!fixtures?.length) {
      throw new TypeError(`cases[${index}].fixtures must be a non-empty array`);
    }
    const oracle = object(item.oracle, `cases[${index}].oracle`);
    const timeoutMs = integer(item.timeoutMs, `cases[${index}].timeoutMs`, 1_000);
    const repetitions = integer(item.repetitions ?? 1, `cases[${index}].repetitions`, 1);
    const acceleration = normalizeAcceleration(id, item.acceleration);
    const cache = item.cache ?? "cold";
    if (cache !== "cold") throw new TypeError("only cold cache is supported");
    return {
      id,
      requirement: requirementFor(id),
      surface: item.surface,
      fixtures,
      oracle,
      timeoutMs,
      repetitions,
      acceleration,
      cache,
    };
  });
  return {
    schemaVersion: 1,
    profileId: input.profileId,
    profileVersion: input.profileVersion,
    cases,
  };
}

export function validateResult(value) {
  const result = object(value, "result");
  if (result.schemaVersion !== 1) {
    throw new TypeError(`unsupported result schema version ${String(result.schemaVersion)}`);
  }
  if (!["running", "passed", "failed", "interrupted"].includes(result.outcome)) {
    throw new TypeError("result.outcome is invalid");
  }
  text(result.profileId, "result.profileId");
  integer(result.profileVersion, "result.profileVersion", 1);
  if (!Array.isArray(result.cases)) throw new TypeError("result.cases must be an array");
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
  if (manifest.schemaVersion !== 1) throw new TypeError("unsupported build manifest schema version");
  if (!manifest.binary || !SHA256.test(manifest.binary.sha256)) {
    throw new TypeError("build manifest binary digest is invalid");
  }
  if (!manifest.driver || !SHA256.test(manifest.driver.sha256)) {
    throw new TypeError("build manifest test-driver digest is invalid");
  }
  for (const [label, artifact] of [["binary", manifest.binary], ["test driver", manifest.driver]]) {
    const name = text(artifact.basename, `build manifest ${label} basename`);
    if (name !== name.split(/[\\/]/).at(-1)) {
      throw new TypeError(`build manifest ${label} basename must not contain a path`);
    }
  }
  if (!Array.isArray(manifest.compileFeatures) || !manifest.compileFeatures.includes("app-e2e")) {
    throw new TypeError("build manifest is not an app-e2e binary");
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
