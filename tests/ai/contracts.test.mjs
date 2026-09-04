import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  normalizeAcceleration,
  requirementsFor,
  validateBuildManifest,
  validateParameters,
  validatePreparedContext,
  validateResult,
} from "./contracts.mjs";

const standard = JSON.parse(readFileSync(new URL("./profiles/standard.json", import.meta.url)));

test("standard profile is valid and omission means no acceleration", () => {
  const parsed = validateParameters(standard);
  assert.deepEqual(parsed.cases.map(({ acceleration }) => acceleration), ["none", "none", "none"]);
  assert.deepEqual(parsed.cases.map(({ requirement }) => requirement), [
    "face-scoring",
    "transcription",
    "transcription",
  ]);
  assert.deepEqual(requirementsFor(standard), ["face-scoring", "transcription"]);
  for (const removed of ["models", "surface", "oracle", "repetitions", "cache"]) {
    assert.equal(parsed.cases.some((item) => removed in item), false);
  }
});

test("schema versions and unsupported accelerators fail before execution", () => {
  assert.throws(() => validateParameters({ ...standard, schemaVersion: 1 }), /unsupported parameter schema/);
  assert.throws(() => normalizeAcceleration("face", "metal"), /does not support/);
  assert.throws(() => normalizeAcceleration("audio-transcription", "cuda"), /does not support/);
  if (!(process.platform === "darwin" && process.arch === "arm64")) {
    assert.throws(() => normalizeAcceleration("audio-transcription", "metal"), /Apple-silicon/);
  }
  assert.throws(() => validateResult({ schemaVersion: 1 }), /unsupported result schema/);
});

test("fixture references cannot carry paths or uppercase hashes", () => {
  const changed = structuredClone(standard);
  changed.cases[0].fixtures[0].basename = "faces/face.jpg";
  assert.throws(() => validateParameters(changed), /must not contain a path/);
  changed.cases[0].fixtures[0].basename = "face.jpg";
  changed.cases[0].fixtures[0].sha256 = "A".repeat(64);
  assert.throws(() => validateParameters(changed), /lowercase hexadecimal/);
});

test("stale or mismatched prepared manifests are rejected", () => {
  const preparedContext = {
    requirements: ["face-scoring", "transcription"],
    artifacts: [
      {
        id: "face-artifact",
        kind: "model",
        requirements: ["face-scoring"],
        readiness: "current",
        identity: { sha256: "c".repeat(64), bytes: 10, version: "face-v1" },
      },
      {
        id: "transcription-artifact",
        kind: "model",
        requirements: ["transcription"],
        readiness: "current",
        identity: { sha256: "d".repeat(64), bytes: 20, version: "speech-v1" },
      },
    ],
    capabilities: [
      { feature: "face-scoring", options: [{ id: "none" }] },
      { feature: "transcription", options: [{ id: "none" }] },
    ],
  };
  const manifest = {
    schemaVersion: 2,
    preparer: { basename: "onecopy-ai-preparer.test", sha256: "a".repeat(64) },
    scenarioExecutable: { basename: "onecopy-ai-scenario.test", sha256: "b".repeat(64) },
    compileFeatures: ["ai-test-support"],
    preparedContext,
  };
  assert.equal(validateBuildManifest(manifest, standard), manifest);
  const missing = structuredClone(manifest);
  missing.preparedContext.requirements.pop();
  assert.throws(() => validateBuildManifest(missing, standard), /invalid requirements|missing requirement/);
  const stale = structuredClone(manifest);
  stale.preparedContext.artifacts[0].readiness = "update-available";
  assert.throws(() => validatePreparedContext(stale.preparedContext), /not current/);
  const wrongFeature = structuredClone(manifest);
  wrongFeature.compileFeatures = [];
  assert.throws(() => validateBuildManifest(wrongFeature, standard), /does not contain AI test support/);
});
