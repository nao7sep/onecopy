import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import viteConfig from "../../vite.config";

// Tauri's `build.devUrl` is the one source of the development endpoint: the
// Vite server and the run-dev launchers both derive it, and the development
// CSP must admit it.
const root = fileURLToPath(new URL("../../", import.meta.url));
const tauri = JSON.parse(readFileSync(`${root}src-tauri/tauri.conf.json`, "utf8")) as {
  build: { devUrl: string };
  app: { security: { devCsp: string } };
};
const devUrl = new URL(tauri.build.devUrl);

describe("development endpoint", () => {
  it("serves Vite on the URL Tauri loads", () => {
    const config = typeof viteConfig === "function"
      ? viteConfig({ command: "serve", mode: "development" })
      : viteConfig;
    expect(config).toMatchObject({
      server: { host: devUrl.hostname, port: Number(devUrl.port), strictPort: true },
    });
  });

  it("gives the launchers the URL Tauri loads", () => {
    const printed = execFileSync(process.execPath, [`${root}scripts/launcher-runtime.mjs`, "dev-url"], {
      encoding: "utf8",
    }).trim();
    expect(new URL(printed).href).toBe(devUrl.href);
  });

  it("lets the development CSP load scripts and hot updates from that URL", () => {
    const directives = new Map(
      tauri.app.security.devCsp.split(";").map((directive) => {
        const [name = "", ...sources] = directive.trim().split(/\s+/);
        return [name, sources] as const;
      }),
    );
    expect(directives.get("script-src")).toContain(devUrl.origin);
    expect(directives.get("connect-src")).toContain(devUrl.origin);
  });
});
