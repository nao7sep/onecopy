import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { arch, cpus, hostname, platform, release } from "node:os";
import { join, resolve } from "node:path";

const npm = process.platform === "win32" ? "npm.cmd" : "npm";
const wdio = resolve(
  process.platform === "win32"
    ? "node_modules/.bin/wdio.cmd"
    : "node_modules/.bin/wdio",
);
const managedRoot = resolve("src-tauri/target/ai-acceptance-home");
const companyRoot = resolve("../company");
const fixtureRoot = join(companyRoot, "assets/test-fixtures");
const env = {
  ...process.env,
  ONECOPY_TEST_MANAGED_ROOT: managedRoot,
  ONECOPY_E2E_HOME: managedRoot,
};
const startedAt = new Date();
const results = [];
const phaseTimeout = 12 * 60 * 60 * 1_000;

function gitRevision(root) {
  const result = spawnSync("git", ["-C", root, "rev-parse", "HEAD"], {
    encoding: "utf8",
    timeout: 10_000,
  });
  return result.status === 0 ? result.stdout.trim() : null;
}

function fixtureDigest(path) {
  return createHash("sha256").update(readFileSync(join(fixtureRoot, path))).digest("hex");
}

const fixturePaths = [
  ...readdirSync(join(fixtureRoot, "photos/faces"))
    .filter((name) => /^face-\d+-(reference|variation)\.jpg$/.test(name))
    .sort()
    .map((name) => `photos/faces/${name}`),
  "audio/dialogue/dialogue-english-with-noise.flac",
  "video/dialogue/dialogue-english-with-noise.mp4",
];

function writeReport(outcome) {
  mkdirSync(managedRoot, { recursive: true });
  const report = {
    schemaVersion: 1,
    outcome,
    startedAt: startedAt.toISOString(),
    finishedAt: new Date().toISOString(),
    machine: {
      hostname: hostname(),
      platform: platform(),
      release: release(),
      arch: arch(),
      logicalCpuCount: cpus().length,
      cpu: cpus()[0]?.model ?? "unknown",
    },
    revisions: {
      onecopy: gitRevision(process.cwd()),
      company: gitRevision(companyRoot),
    },
    fixtures: fixturePaths.map((path) => ({ path, sha256: fixtureDigest(path) })),
    results,
  };
  writeFileSync(
    resolve(managedRoot, "acceptance-result.json"),
    `${JSON.stringify(report, null, 2)}\n`,
  );
}

function run(label, command, args, extraEnv = {}) {
  console.log(`\n> ${command} ${args.join(" ")}`);
  const started = Date.now();
  const result = spawnSync(command, args, {
    cwd: process.cwd(),
    env: { ...env, ...extraEnv },
    stdio: "inherit",
    timeout: phaseTimeout,
  });
  const status = result.status ?? 1;
  results.push({
    label,
    status,
    elapsedMs: Date.now() - started,
    ...(result.signal ? { signal: result.signal } : {}),
    ...(result.error ? { error: result.error.message } : {}),
  });
  if (status !== 0) {
    writeReport("failed");
    if (result.error) console.error(result.error.message);
    process.exit(status);
  }
}

console.log(`AI acceptance artifacts are retained in ${managedRoot}`);
console.log(`${platform()} ${release()} ${arch()} — ${cpus()[0]?.model ?? "unknown"}`);
run("face models", "cargo", [
  "test", "--manifest-path", "src-tauri/Cargo.toml", "--test", "face_tests",
  "live_face_models_score_canonical_company_fixtures", "--", "--ignored", "--exact",
  "--nocapture", "--test-threads=1",
]);
run("Whisper engine control", "cargo", [
  "test", "--manifest-path", "src-tauri/Cargo.toml", "--test", "transcription_tests",
  "live_tiny_model_transcribes_the_canonical_sample", "--", "--ignored", "--exact",
  "--nocapture", "--test-threads=1",
]);
run("production transcription model", "cargo", [
  "test", "--manifest-path", "src-tauri/Cargo.toml", "--test", "transcription_tests",
  "live_production_model_transcribes_canonical_audio_and_video_without_loops", "--",
  "--ignored", "--exact", "--nocapture", "--test-threads=1",
]);
run("running-app build", npm, ["run", "build:e2e"]);
run("running-app fixture setup", "node", ["scripts/prepare-e2e.mjs"], {
  ONECOPY_AI_ACCEPTANCE: "1",
});
run("running-app AI journey", wdio, ["run", "wdio.conf.mjs"], {
  ONECOPY_AI_ACCEPTANCE: "1",
});
writeReport("passed");
console.log(`\nPASS — report: ${resolve(managedRoot, "acceptance-result.json")}`);
