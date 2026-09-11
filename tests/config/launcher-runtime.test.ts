import { randomUUID } from "node:crypto";
import { describe, expect, it } from "vitest";

// The launcher helper is plain ESM because both shell families execute it directly.
// @ts-expect-error The directly executed .mjs helper intentionally has no declaration file.
import { claimRuntime, isRuntimeOwner, ownsProcess, releaseRuntime, REPO_ROOT } from "../../scripts/launcher-runtime.mjs";

describe("launcher process identity", () => {
  const identity = { kind: "tauri", label: "OneCopy", executable: "onecopy" };

  it("owns only this repository's OneCopy runtime", () => {
    expect(ownsProcess({
      pid: 10,
      parentPid: 1,
      executablePath: `${REPO_ROOT}/src-tauri/target/debug/onecopy`,
      commandLine: "",
    }, identity)).toBe(true);
    expect(ownsProcess({
      pid: 11,
      parentPid: 1,
      executablePath: `${REPO_ROOT}/src-tauri/target/release/bundle/macos/OneCopy.app/Contents/MacOS/onecopy`,
      commandLine: "",
    }, identity)).toBe(true);
    expect(ownsProcess({
      pid: 12,
      parentPid: 1,
      executablePath: "/another/repo/src-tauri/target/debug/onecopy",
      commandLine: "",
    }, identity)).toBe(false);
  });

  it("does not let a stale launcher generation clean up its replacement", async () => {
    const first = `first-${randomUUID()}`;
    const second = `second-${randomUUID()}`;
    try {
      await claimRuntime(first);
      await claimRuntime(second);
      expect(await isRuntimeOwner(first)).toBe(false);
      expect(await isRuntimeOwner(second)).toBe(true);
    } finally {
      await releaseRuntime(second);
    }
  });
});
