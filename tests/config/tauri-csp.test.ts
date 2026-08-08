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
  it("is present and non-empty", () => {
    expect(typeof csp).toBe("string");
    expect((csp as string).trim().length).toBeGreaterThan(0);
  });

  it("never allows 'unsafe-eval' anywhere in the policy", () => {
    expect(csp as string).not.toContain("'unsafe-eval'");
  });

  it("keeps script-src strict: no 'unsafe-inline' and no 'unsafe-eval'", () => {
    // Isolate the script-src directive (up to the next `;` or end of string).
    const scriptSrc = /script-src\b[^;]*/.exec(csp as string)?.[0] ?? "";
    expect(scriptSrc).not.toBe("");
    expect(scriptSrc).not.toContain("'unsafe-inline'");
    expect(scriptSrc).not.toContain("'unsafe-eval'");
    expect(scriptSrc).toContain("'self'");
  });

  it("matches the snapshotted production policy exactly", () => {
    expect(csp).toBe(EXPECTED_CSP);
  });
});
