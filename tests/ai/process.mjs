import { execFile, execFileSync, spawn } from "node:child_process";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

async function processRows() {
  try {
    if (process.platform === "win32") {
      const { stdout: raw } = await execFileAsync(
        "powershell",
        ["-NoProfile", "-Command", "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,WorkingSetSize | ConvertTo-Json -Compress"],
        { encoding: "utf8", timeout: 15_000, windowsHide: true, maxBuffer: 16 * 1024 * 1024 },
      );
      const parsed = JSON.parse(raw);
      return (Array.isArray(parsed) ? parsed : [parsed]).map((row) => ({
        pid: Number(row.ProcessId),
        parent: Number(row.ParentProcessId),
        bytes: Number(row.WorkingSetSize ?? 0),
      }));
    }
    const { stdout } = await execFileAsync("ps", ["-axo", "pid=,ppid=,rss="], {
      encoding: "utf8",
      timeout: 15_000,
      maxBuffer: 16 * 1024 * 1024,
    });
    return stdout
      .trim()
      .split(/\r?\n/)
      .map((line) => line.trim().split(/\s+/).map(Number))
      .map(([pid, parent, kib]) => ({ pid, parent, bytes: kib * 1024 }));
  } catch {
    return null;
  }
}

async function treeBytes(rootPid) {
  const rows = await processRows();
  if (!rows?.some(({ pid }) => pid === rootPid)) return null;
  const included = new Set([rootPid]);
  let changed = true;
  while (changed) {
    changed = false;
    for (const row of rows) {
      if (included.has(row.parent) && !included.has(row.pid)) {
        included.add(row.pid);
        changed = true;
      }
    }
  }
  return rows.filter(({ pid }) => included.has(pid)).reduce((sum, row) => sum + row.bytes, 0);
}

async function rootBytes(rootPid) {
  if (process.platform !== "win32") return null;
  try {
    const { stdout } = await execFileAsync(
      "tasklist",
      ["/FI", `PID eq ${rootPid}`, "/FO", "CSV", "/NH"],
      { encoding: "utf8", timeout: 5_000, windowsHide: true },
    );
    const lastField = stdout.trim().match(/"([^"]*)"\s*$/)?.[1] ?? "";
    const kib = Number(lastField.replace(/\D/g, ""));
    return Number.isFinite(kib) && kib > 0 ? kib * 1024 : null;
  } catch {
    return null;
  }
}

function terminateTree(child) {
  if (child.exitCode !== null) return null;
  if (process.platform === "win32") {
    try {
      execFileSync("taskkill", ["/PID", String(child.pid), "/T"], {
        stdio: "ignore",
        timeout: 30_000,
        windowsHide: true,
      });
    } catch {
      child.kill();
    }
    return setTimeout(() => {
      if (child.exitCode !== null) return;
      try {
        execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], {
          stdio: "ignore",
          timeout: 30_000,
          windowsHide: true,
        });
      } catch {
        child.kill();
      }
    }, 5_000);
  } else {
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      child.kill("SIGTERM");
    }
    return setTimeout(() => {
      if (child.exitCode !== null) return;
      try {
        process.kill(-child.pid, "SIGKILL");
      } catch {
        child.kill("SIGKILL");
      }
    }, 5_000);
  }
}

export function runOwned(
  command,
  args,
  {
    cwd,
    env = {},
    timeoutMs,
    measureMemory = true,
    onStdout = () => {},
    onStderr = () => {},
    signal,
  },
) {
  return new Promise((resolve, reject) => {
    const started = process.hrtime.bigint();
    const child = spawn(command, args, {
      cwd,
      env: { ...process.env, ...env },
      windowsHide: true,
      detached: process.platform !== "win32",
      stdio: ["ignore", "pipe", "pipe"],
    });
    const launchMs = Number(process.hrtime.bigint() - started) / 1_000_000;
    let stdout = "";
    let stderr = "";
    let peakProcessTreeBytes = null;
    let treeSampling = Promise.resolve();
    let rootSampling = Promise.resolve();
    let treeBusy = false;
    let rootBusy = false;
    let timedOut = false;
    let interrupted = false;
    let forceKill = null;
    let terminationRequested = false;
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
      onStdout(chunk);
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
      onStderr(chunk);
    });
    const sampleTree = () => {
      if (treeBusy) return;
      treeBusy = true;
      treeSampling = (async () => {
        const bytes = await treeBytes(child.pid);
        if (Number.isFinite(bytes)) {
          peakProcessTreeBytes = peakProcessTreeBytes === null
            ? bytes
            : Math.max(peakProcessTreeBytes, bytes);
        }
        treeBusy = false;
      })();
    };
    const sampleRoot = () => {
      if (rootBusy) return;
      rootBusy = true;
      rootSampling = (async () => {
        const bytes = await rootBytes(child.pid);
        if (Number.isFinite(bytes)) {
          peakProcessTreeBytes = peakProcessTreeBytes === null
            ? bytes
            : Math.max(peakProcessTreeBytes, bytes);
        }
        rootBusy = false;
      })();
    };
    if (measureMemory) {
      sampleTree();
      sampleRoot();
    }
    const treeSample = measureMemory ? setInterval(sampleTree, 2_000) : null;
    const rootSample = measureMemory ? setInterval(sampleRoot, 250) : null;
    const requestTermination = () => {
      if (terminationRequested || child.exitCode !== null) return;
      terminationRequested = true;
      forceKill = terminateTree(child);
    };
    const timeout = setTimeout(() => {
      timedOut = true;
      requestTermination();
    }, timeoutMs);
    const interrupt = () => {
      interrupted = true;
      requestTermination();
    };
    const abort = () => interrupt();
    process.once("SIGINT", interrupt);
    process.once("SIGTERM", interrupt);
    signal?.addEventListener("abort", abort, { once: true });
    if (signal?.aborted) abort();
    child.once("error", (error) => {
      if (treeSample) clearInterval(treeSample);
      if (rootSample) clearInterval(rootSample);
      clearTimeout(timeout);
      if (forceKill) clearTimeout(forceKill);
      process.off("SIGINT", interrupt);
      process.off("SIGTERM", interrupt);
      signal?.removeEventListener("abort", abort);
      reject(error);
    });
    child.once("close", async (code, exitSignal) => {
      if (treeSample) clearInterval(treeSample);
      if (rootSample) clearInterval(rootSample);
      clearTimeout(timeout);
      if (forceKill) clearTimeout(forceKill);
      process.off("SIGINT", interrupt);
      process.off("SIGTERM", interrupt);
      signal?.removeEventListener("abort", abort);
      if (measureMemory) await Promise.allSettled([treeSampling, rootSampling]);
      resolve({
        code: code ?? 1,
        signal: exitSignal,
        timedOut,
        interrupted,
        stdout,
        stderr,
        launchMs,
        wallMs: Number(process.hrtime.bigint() - started) / 1_000_000,
        peakProcessTreeBytes,
      });
    });
  });
}
