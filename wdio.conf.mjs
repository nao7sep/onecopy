import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const benchmarkCase = process.env.ONECOPY_AI_BENCHMARK_CASE;
const binary = process.env.ONECOPY_E2E_BINARY ?? resolve(
  process.platform === "win32"
    ? "src-tauri/target/debug/onecopy.exe"
    : "src-tauri/target/debug/onecopy",
);
const home = process.env.ONECOPY_E2E_HOME ??
  join(tmpdir(), "onecopy-wdio-acceptance");
process.env.ONECOPY_E2E_HOME = home;
process.env.ONECOPY_HOME = home;

export const config = {
  runner: "local",
  specs: [
    benchmarkCase
      ? "./tests/ai/app-case.e2e.mjs"
      : "./tests/acceptance/initialization.e2e.mjs",
  ],
  maxInstances: 1,
  capabilities: [
    {
      browserName: "tauri",
      "tauri:options": { application: binary },
    },
  ],
  services: [
    [
      "@wdio/tauri-service",
      {
        appBinaryPath: binary,
        driverProvider: "embedded",
        embeddedPort: 4445,
        startTimeout: 5 * 60_000,
      },
    ],
  ],
  framework: "mocha",
  reporters: ["spec"],
  logLevel: "warn",
  waitforTimeout: benchmarkCase
    ? Number(process.env.ONECOPY_AI_CASE_TIMEOUT_MS)
    : 5 * 60_000,
  connectionRetryTimeout: 5 * 60_000,
  connectionRetryCount: 1,
  mochaOpts: {
    ui: "bdd",
    timeout: benchmarkCase
      ? Number(process.env.ONECOPY_AI_CASE_TIMEOUT_MS)
      : 10 * 60_000,
  },
};
