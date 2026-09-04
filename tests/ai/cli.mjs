import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { compare } from "./compare.mjs";
import { prepare } from "./prepare.mjs";
import { requireUnusedReportPath, safeConsoleMessage } from "./report.mjs";
import { runScenarios } from "./scenario-runner.mjs";

function options(args) {
  const parsed = { positionals: [] };
  for (let index = 0; index < args.length; index += 1) {
    const value = args[index];
    if (!value.startsWith("--")) {
      parsed.positionals.push(value);
      continue;
    }
    const key = value.slice(2);
    const next = args[index + 1];
    if (!next || next.startsWith("--")) throw new Error(`missing value for --${key}`);
    parsed[key] = next;
    index += 1;
  }
  return parsed;
}

function usage() {
  console.log(`OneCopy AI integration harness

  npm run test:ai:prepare -- [--parameters FILE] [--fixtures DIR] [--prepared DIR] [--reuse-managed DIR]
  npm run test:ai:live -- [--parameters FILE] [--fixtures DIR] [--prepared DIR] [--report FILE]
  npm run test:ai:benchmark -- [--parameters FILE] [--fixtures DIR] [--prepared DIR] [--report FILE]
  npm run test:ai:compare -- LEFT.json RIGHT.json

Benchmark acceleration is explicit in the parameter file; omission means CPU-only.
Preparation may download/build. Live and benchmark execution are offline and never do either.`);
}

function timestampForFile(now = new Date()) {
  const iso = now.toISOString();
  return `${iso.slice(0, 10).replaceAll("-", "")}-${iso.slice(11, 19).replaceAll(":", "")}-${iso.slice(20, 23)}-utc`;
}

const repositoryRoot = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const action = process.argv[2];
if (!action || action === "help" || action === "--help") {
  usage();
  process.exit(0);
}

try {
  const parsed = options(process.argv.slice(3));
  if (action === "compare") {
    if (parsed.positionals.length !== 2) throw new Error("compare requires two result files");
    console.log(JSON.stringify(compare(resolve(parsed.positionals[0]), resolve(parsed.positionals[1])), null, 2));
    process.exit(0);
  }
  const defaults = {
    repositoryRoot,
    parameterPath: resolve(
      parsed.parameters ?? "tests/ai/profiles/standard.json",
    ),
    fixtureRoot: resolve(parsed.fixtures ?? "../company/assets/test-fixtures"),
    preparedRoot: resolve(parsed.prepared ?? "src-tauri/target/ai-benchmark"),
    reuseManagedRoot: parsed["reuse-managed"]
      ? resolve(parsed["reuse-managed"])
      : undefined,
  };
  if (!existsSync(defaults.parameterPath)) throw new Error("parameter file does not exist");
  if (!existsSync(defaults.fixtureRoot)) throw new Error("fixture root does not exist");
  if (action === "prepare") {
    const result = await prepare(defaults);
    console.log(`Prepared AI artifacts and scenario executables (${result.parameters.profileId}).`);
  } else if (action === "live" || action === "benchmark") {
    const reportPath = resolve(
      parsed.report ??
        `artifacts/ai-benchmark/${action}-${timestampForFile()}.json`,
    );
    requireUnusedReportPath(reportPath);
    const result = await runScenarios({
      ...defaults,
      reportPath,
      observe: action === "benchmark",
    });
    console.log(`${action} ${result.outcome}; result file: ${reportPath.split(/[\\/]/).at(-1)}`);
    if (result.outcome !== "passed") process.exitCode = 1;
  } else {
    usage();
    process.exitCode = 2;
  }
} catch (error) {
  console.error(safeConsoleMessage(error));
  process.exitCode = 1;
}
