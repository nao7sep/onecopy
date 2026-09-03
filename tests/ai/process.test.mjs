import assert from "node:assert/strict";
import test from "node:test";
import { runOwned } from "./process.mjs";

test("owned child reports monotonic wall time and output", async () => {
  const result = await runOwned(process.execPath, ["-e", "process.stdout.write('ready')"], {
    cwd: process.cwd(),
    timeoutMs: 10_000,
  });
  assert.equal(result.code, 0);
  assert.equal(result.stdout, "ready");
  assert(result.wallMs > 0);
  assert(result.peakProcessTreeBytes >= 0);
});

test("owned child timeout is terminal", async () => {
  const result = await runOwned(process.execPath, ["-e", "setInterval(() => {}, 1000)"], {
    cwd: process.cwd(),
    timeoutMs: 200,
  });
  assert.equal(result.timedOut, true);
  assert.notEqual(result.code, 0);
});
