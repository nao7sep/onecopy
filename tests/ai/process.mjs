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
    return [];
  }
}

async function treeBytes(rootPid) {
  const rows = await processRows();
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

function terminateTree(child) {
  if (child.exitCode !== null) return;
  if (process.platform === "win32") {
    try {
      execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], {
        stdio: "ignore",
        timeout: 30_000,
        windowsHide: true,
      });
    } catch {
      child.kill();
    }
  } else {
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      child.kill("SIGTERM");
    }
  }
}

export function runOwned(command, args, { cwd, env = {}, timeoutMs }) {
  return new Promise((resolve, reject) => {
    const started = process.hrtime.bigint();
    const child = spawn(command, args, {
      cwd,
      env: { ...process.env, ...env },
      windowsHide: true,
      detached: process.platform !== "win32",
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    let peakProcessTreeBytes = 0;
    let sampling = false;
    let timedOut = false;
    let interrupted = false;
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => { stdout += chunk; });
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    const sample = setInterval(async () => {
      if (sampling) return;
      sampling = true;
      peakProcessTreeBytes = Math.max(peakProcessTreeBytes, await treeBytes(child.pid));
      sampling = false;
    }, 1_000);
    const timeout = setTimeout(() => {
      timedOut = true;
      terminateTree(child);
    }, timeoutMs);
    const interrupt = () => {
      interrupted = true;
      terminateTree(child);
    };
    process.once("SIGINT", interrupt);
    process.once("SIGTERM", interrupt);
    child.once("error", (error) => {
      clearInterval(sample);
      clearTimeout(timeout);
      process.off("SIGINT", interrupt);
      process.off("SIGTERM", interrupt);
      reject(error);
    });
    child.once("close", (code, signal) => {
      clearInterval(sample);
      clearTimeout(timeout);
      process.off("SIGINT", interrupt);
      process.off("SIGTERM", interrupt);
      resolve({
        code: code ?? 1,
        signal,
        timedOut,
        interrupted,
        stdout,
        stderr,
        wallMs: Number(process.hrtime.bigint() - started) / 1_000_000,
        peakProcessTreeBytes,
      });
    });
  });
}
