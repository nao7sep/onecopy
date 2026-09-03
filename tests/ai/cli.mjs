import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { runBenchmark } from "./benchmark.mjs";
import { compare } from "./compare.mjs";
import { runLive } from "./live.mjs";
import { prepare } from "./prepare.mjs";
import { recoverInterruptedReport, safeFailure } from "./report.mjs";

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
  console.log(`OneCopy AI test system

  npm run test:ai:prepare -- [--parameters FILE] [--fixtures DIR] [--prepared DIR] [--reuse-managed DIR]
  npm run test:ai:live -- [--parameters FILE] [--fixtures DIR] [--prepared DIR] [--report FILE]
  npm run test:ai:benchmark -- [--parameters FILE] [--fixtures DIR] [--prepared DIR] [--report FILE]
  npm run test:ai:compare -- LEFT.json RIGHT.json

Benchmark acceleration is explicit in the parameter file; omission means CPU-only.
Preparation may download/build. Live and benchmark execution are offline and never do either.`);
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
      parsed.parameters ??
        (action === "live" ? "tests/ai/profiles/live.json" : "tests/ai/profiles/standard.json"),
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
    console.log(`Prepared AI artifacts and application (${result.parameters.profileId}).`);
  } else if (action === "live" || action === "benchmark") {
    const reportPath = resolve(
      parsed.report ??
        `artifacts/ai-benchmark/${action}-${new Date().toISOString().replace(/[:.]/g, "-")}.json`,
    );
    if (recoverInterruptedReport(reportPath)) {
      throw new Error("the prior running result was sealed as interrupted; choose a new report file");
    }
    const result = action === "live"
      ? await runLive({ ...defaults, reportPath })
      : await runBenchmark({ ...defaults, reportPath });
    console.log(`${action} ${result.outcome}; result file: ${reportPath.split(/[\\/]/).at(-1)}`);
    if (result.outcome !== "passed") process.exitCode = 1;
  } else {
    usage();
    process.exitCode = 2;
  }
} catch (error) {
  console.error(safeFailure("test-system", error).message);
  process.exitCode = 1;
}
