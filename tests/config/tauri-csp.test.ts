import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

// Read the shipped Tauri config and guard the PRODUCTION Content-Security-Policy.
// A build that silently drops or weakens `app.security.csp` (e.g. a tooling
// regression collapsing it to null, or someone slipping in 'unsafe-inline' /
// 'unsafe-eval' for scripts) would let injected markup execute in the webview.
// This is a render-free check on the config text.
const config = JSON.parse(
  readFileSync(
    fileURLToPath(new URL("../../src-tauri/tauri.conf.json", import.meta.url)),
    "utf8",
  ),
) as { app?: { security?: { csp?: unknown } } };

const csp = config.app?.security?.csp;

// The exact production CSP, snapshotted so any future drop or weakening fails.
// Keep this in lock-step with src-tauri/tauri.conf.json → app.security.csp;
// a deliberate policy change updates both, an accidental one trips this test.
const EXPECTED_CSP =
  "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' asset: http://asset.localhost mediacache: http://mediacache.localhost mediafile: http://mediafile.localhost data: blob:; media-src 'self' mediafile: http://mediafile.localhost; font-src 'self' data:; connect-src 'self' ipc: http://ipc.localhost; object-src 'none'; base-uri 'self'; frame-ancestors 'none'";

describe("Tauri production CSP (src-tauri/tauri.conf.json)", () => {
  // The exact-match case below subsumes present/non-empty, no-unsafe-eval and
  // strict-script-src: any of those regressions changes the string. What it
  // could NOT see is devCsp, which legitimately carries 'unsafe-eval' and
  // 'unsafe-inline' for the Vite dev server — so the one thing worth asserting
  // separately is that the two never get confused.
  it("matches the snapshotted production policy exactly", () => {
    expect(csp).toBe(EXPECTED_CSP);
  });

  it("keeps the permissive dev policy out of the production one", () => {
    const devCsp = (config.app?.security as { devCsp?: unknown } | undefined)?.devCsp;
    expect(typeof devCsp, "devCsp should still exist for the dev server").toBe(
      "string",
    );
    // The dev policy is permissive by necessity; production must not be.
    expect(devCsp as string).toContain("'unsafe-eval'");
    expect(csp as string).not.toContain("'unsafe-eval'");
    expect(csp).not.toBe(devCsp);
    // A production build must not be reachable from the dev origin either.
    expect(csp as string).not.toContain("localhost:1721");
  });
});
