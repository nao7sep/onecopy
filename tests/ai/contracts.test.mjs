import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  dependenciesFor,
  normalizeAcceleration,
  validateBuildManifest,
  validateParameters,
  validateResult,
} from "./contracts.mjs";

const standard = JSON.parse(readFileSync(new URL("./profiles/standard.json", import.meta.url)));

test("standard profile is valid and omission means no acceleration", () => {
  const parsed = validateParameters(standard);
  assert.deepEqual(parsed.cases.map(({ acceleration }) => acceleration), ["none", "none", "none"]);
  assert.deepEqual(dependenciesFor(standard), [
    ...(process.platform === "win32" ? ["onnxruntime-win-x64"] : []),
    "ultraface-rfb640",
    "hsemotion-enet-b2",
    "ffmpeg",
    "whisper-large-v3-turbo",
  ]);
});

test("schema versions and unsupported accelerators fail before execution", () => {
  assert.throws(() => validateParameters({ ...standard, schemaVersion: 2 }), /unsupported parameter schema/);
  assert.throws(() => normalizeAcceleration("face", "metal"), /does not support/);
  assert.throws(() => normalizeAcceleration("audio-transcription", "cuda"), /does not support/);
  if (!(process.platform === "darwin" && process.arch === "arm64")) {
    assert.throws(() => normalizeAcceleration("audio-transcription", "metal"), /Apple-silicon/);
  }
  assert.throws(() => validateResult({ schemaVersion: 2 }), /unsupported result schema/);
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
  const manifest = {
    schemaVersion: 1,
    binary: { basename: "onecopy.test", sha256: "a".repeat(64) },
    driver: { basename: "onecopy-driver.test", sha256: "b".repeat(64) },
    compileFeatures: ["app-e2e"],
    accelerationCapabilities: [
      { feature: "face-scoring", modes: ["none"] },
      { feature: "transcription", modes: ["none"] },
    ],
    dependencies: dependenciesFor(standard).map((id) => ({ id })),
  };
  assert.equal(validateBuildManifest(manifest, standard), manifest);
  const missing = structuredClone(manifest);
  missing.dependencies.pop();
  assert.throws(() => validateBuildManifest(missing, standard), /missing dependency/);
  const wrongFeature = structuredClone(manifest);
  wrongFeature.compileFeatures = [];
  assert.throws(() => validateBuildManifest(wrongFeature, standard), /not an app-e2e/);
});
