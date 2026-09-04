import assert from "node:assert/strict";
import test from "node:test";
import { runOwned } from "./process.mjs";

test("owned child reports monotonic wall time, output, and memory", async () => {
  const result = await runOwned(process.execPath, ["-e", "setTimeout(() => process.stdout.write('ready'), 3000)"], {
    cwd: process.cwd(),
    timeoutMs: 10_000,
  });
  assert.equal(result.code, 0);
  assert.equal(result.stdout, "ready");
  assert(result.wallMs > 0);
  assert(result.peakProcessTreeBytes > 0);
});

test("owned child timeout is terminal", async () => {
  const result = await runOwned(process.execPath, ["-e", "setInterval(() => {}, 1000)"], {
    cwd: process.cwd(),
    timeoutMs: 200,
  });
  assert.equal(result.timedOut, true);
  assert.notEqual(result.code, 0);
});

test("memory is null when observation is not requested", async () => {
  const result = await runOwned(process.execPath, ["-e", "process.exit(0)"], {
    cwd: process.cwd(),
    timeoutMs: 5_000,
    measureMemory: false,
  });
  assert.equal(result.peakProcessTreeBytes, null);
});

test("an uncooperative POSIX child escalates from TERM to KILL", {
  skip: process.platform === "win32",
}, async () => {
  const result = await runOwned(
    process.execPath,
    ["-e", "process.on('SIGTERM', () => {}); setInterval(() => {}, 1000)"],
    { cwd: process.cwd(), timeoutMs: 100, measureMemory: false },
  );
  assert.equal(result.timedOut, true);
  assert.equal(result.signal, "SIGKILL");
  assert(result.wallMs >= 5_000 && result.wallMs < 10_000, `wallMs=${result.wallMs}`);
});
