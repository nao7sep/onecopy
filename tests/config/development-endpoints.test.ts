import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const root = fileURLToPath(new URL("../../", import.meta.url));
const read = (path: string): string => readFileSync(root + path, "utf8");
const tauri = JSON.parse(read("src-tauri/tauri.conf.json")) as {
  build: { devUrl: string };
  app: { security: { devCsp: string } };
};

describe("development endpoints", () => {
  it("keeps Vite, HMR, Tauri, and both launchers on the app-owned ports", () => {
    expect(read("vite.config.ts")).toContain("port: 28867");
    expect(read("vite.config.ts")).toContain("port: 29587");
    expect(read("vite.config.ts")).toContain('remoteHost ?? "127.0.0.1"');
    expect(tauri.build.devUrl).toBe("http://127.0.0.1:28867");
    expect(tauri.app.security.devCsp).toContain("http://127.0.0.1:28867");
    for (const path of ["scripts/run-dev.command", "scripts/run-dev.ps1"]) {
      expect(read(path)).toContain("28867");
      expect(read(path)).toContain("29587");
      expect(read(path)).not.toMatch(/stop[_-]port|Stop-Port/);
    }
  });

  it("gives a cold Windows native build a bounded readiness interval", () => {
    const launcher = read("scripts/run-dev.ps1");
    expect(launcher).toContain("$sourceReadyTimeoutMs = 900000");
    expect(launcher).toContain('"wait-process", (Join-Path $repoDir "src-tauri/target/debug/onecopy.exe"), $sourceReadyTimeoutMs');
  });
});
