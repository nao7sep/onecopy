import { mkdirSync, renameSync, writeFileSync } from "node:fs";
import { basename, dirname } from "node:path";

const FORBIDDEN_KEYS = /(host.?name|user.?name|absolute.?path|(^|_)path$|home.?dir|environment|command.?line|git.?remote|transcript|embedding|raw.?content|serial|drive)/i;
const PATH_VALUE = /(?:^[a-z]:[\\/]|^\\\\|^\/(?:Users|home|var|tmp|private|opt|mnt)\/|[\\/](?:Users|home)[\\/])/i;

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

export function safeFailure(category, error) {
  const message = String(error instanceof Error ? error.message : error)
    .replace(/[a-z]:[\\/][^;\n]*/gi, "<local-path>")
    .replace(/\/(?:Users|home|var|tmp|private|opt|mnt)\/[^;\n]*/gi, "<local-path>");
  return { category, message };
}

export function writeAtomicReport(path, report) {
  assertPrivacySafe(report);
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.partial`;
  writeFileSync(temporary, `${JSON.stringify(report, null, 2)}\n`, { flag: "wx" });
  renameSync(temporary, path);
  return basename(path);
}

export function compatibleResults(left, right) {
  const fields = ["schemaVersion", "profileId", "profileVersion"];
  for (const field of fields) {
    if (left[field] !== right[field]) throw new Error(`results differ at ${field}`);
  }
  const identity = (result) => result.cases.map(({ id, dependencies, fixtures }) => ({
    id,
    dependencies,
    fixtures,
  }));
  if (JSON.stringify(identity(left)) !== JSON.stringify(identity(right))) {
    throw new Error("results use different cases, dependencies, or fixtures");
  }
  if (JSON.stringify(left.source) !== JSON.stringify(right.source)) {
    throw new Error("results use different source states");
  }
  if (left.binary?.sha256 !== right.binary?.sha256) {
    throw new Error("results use different application binaries");
  }
  return true;
}
