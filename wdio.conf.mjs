import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const binary = resolve(
  process.platform === "win32"
    ? "src-tauri/target/debug/onecopy.exe"
    : "src-tauri/target/debug/onecopy",
);
const aiAcceptance = process.env.ONECOPY_AI_ACCEPTANCE === "1";
const home = process.env.ONECOPY_E2E_HOME ??
  (aiAcceptance
    ? resolve("src-tauri/target/ai-acceptance-home")
    : join(tmpdir(), "onecopy-wdio-acceptance"));
process.env.ONECOPY_E2E_HOME = home;
process.env.ONECOPY_HOME = home;

export const config = {
  runner: "local",
  specs: [
    aiAcceptance
      ? "./tests/acceptance/ai-features.e2e.mjs"
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
  waitforTimeout: aiAcceptance ? 12 * 60 * 60 * 1_000 : 5 * 60_000,
  connectionRetryTimeout: 5 * 60_000,
  connectionRetryCount: 1,
  mochaOpts: {
    ui: "bdd",
    timeout: aiAcceptance ? 12 * 60 * 60 * 1_000 : 10 * 60_000,
  },
};
