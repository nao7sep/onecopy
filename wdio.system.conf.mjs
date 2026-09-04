import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const binary = process.env.ONECOPY_SYSTEM_BINARY ?? resolve(
  process.platform === "win32"
    ? "src-tauri/target/debug/onecopy.exe"
    : "src-tauri/target/debug/onecopy",
);
const home = process.env.ONECOPY_SYSTEM_HOME ??
  join(tmpdir(), "onecopy-wdio-system");
process.env.ONECOPY_SYSTEM_HOME = home;
process.env.ONECOPY_HOME = home;

export const config = {
  runner: "local",
  specs: ["./tests/system/initialization.system.mjs"],
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
  waitforTimeout: 5 * 60_000,
  connectionRetryTimeout: 5 * 60_000,
  connectionRetryCount: 1,
  mochaOpts: {
    ui: "bdd",
    timeout: 10 * 60_000,
  },
};
