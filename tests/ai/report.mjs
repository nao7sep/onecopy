import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname } from "node:path";
import { validateResult } from "./contracts.mjs";

const FORBIDDEN_KEYS = /(host.?name|user.?name|absolute.?path|(^|_)path$|home.?dir|environment|command.?line|git.?remote|transcript|embedding|raw.?content|serial|drive.?(identity|id))/i;
const PATH_VALUE = /(?:^|[\s'":(])(?:[a-z]:[\\/]|\\\\|\/(?!\/))|[\\/](?:Users|home)[\\/]/i;

export function assertPrivacySafe(value, location = "result") {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => assertPrivacySafe(entry, `${location}[${index}]`));
    return value;
  }
  if (value && typeof value === "object") {
    for (const [key, entry] of Object.entries(value)) {
      if (FORBIDDEN_KEYS.test(key)) throw new Error(`${location}.${key} is prohibited`);
      assertPrivacySafe(entry, `${location}.${key}`);
    }
    return value;
  }
  if (typeof value === "string" && PATH_VALUE.test(value)) {
    throw new Error(`${location} contains a path-shaped value`);
  }
  return value;
}

export function safeConsoleMessage(error) {
  const message = String(error instanceof Error ? error.message : error)
    .replace(/[\u0000-\u001f\u007f]+/g, " ")
    .trim();
  if (PATH_VALUE.test(message)) {
    return "The command could not finish because a local path was unavailable.";
  }
  return message || "The command could not finish.";
}

export function writeAtomicReport(path, report) {
  assertPrivacySafe(report);
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.partial`;
  try {
    writeFileSync(temporary, `${JSON.stringify(report, null, 2)}\n`, { flag: "wx" });
    renameSync(temporary, path);
  } finally {
    if (existsSync(temporary)) {
      rmSync(temporary, { force: true });
    }
  }
  return basename(path);
}

export function recoverInterruptedReport(path) {
  if (!existsSync(path)) return false;
  const prior = JSON.parse(readFileSync(path, "utf8"));
  if (prior.outcome !== "running") return false;
  prior.outcome = "interrupted";
  prior.finishedAtUtc = new Date().toISOString();
  for (const item of prior.cases ?? []) {
    if (item.outcome !== "running") continue;
    item.outcome = "interrupted";
    item.finishedAtUtc = prior.finishedAtUtc;
    item.failure = {
      category: "runner-interrupted",
      message: "The prior runner exited before recording this scenario's terminal state.",
    };
  }
  prior.failure = {
    category: "runner-interrupted",
    message: "The prior runner exited before recording its terminal state.",
  };
  writeAtomicReport(path, validateResult(prior));
  return true;
}

export function requireUnusedReportPath(path) {
  if (!existsSync(path)) return;
  if (recoverInterruptedReport(path)) {
    throw new Error("the prior running result was sealed as interrupted; choose a new report file");
  }
  throw new Error("the report file already exists; choose a new report file");
}

export function compatibleResults(left, right) {
  const fields = ["schemaVersion", "profileId", "profileVersion", "mode", "buildManifestSha256"];
  for (const field of fields) {
    if (left[field] !== right[field]) throw new Error(`results differ at ${field}`);
  }
  const identity = (result) => result.cases.map(({ scenarioId, dependencies, fixtures }) => ({
    scenarioId,
    dependencies,
    fixtures,
  }));
  if (JSON.stringify(identity(left)) !== JSON.stringify(identity(right))) {
    throw new Error("results use different cases, dependencies, or fixtures");
  }
  if (JSON.stringify(left.source) !== JSON.stringify(right.source)) {
    throw new Error("results use different source states");
  }
  if (JSON.stringify(left.executable) !== JSON.stringify(right.executable)) {
    throw new Error("results use different scenario executables");
  }
  return { machineFactsMatch: JSON.stringify(left.machine) === JSON.stringify(right.machine) };
}
