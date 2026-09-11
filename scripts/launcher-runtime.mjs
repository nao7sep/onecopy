import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { readFile, unlink, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { dirname, join, normalize, relative, resolve } from "node:path";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT = resolve(SCRIPT_DIR, "..");
const OWNER_FILE = join(
  tmpdir(),
  `company-launcher-${createHash("sha256").update(REPO_ROOT).digest("hex").slice(0, 20)}.owner`,
);

function normalized(value) {
  return normalize(value ?? "").replaceAll("\\", "/").toLowerCase();
}

function underRoot(value) {
  const candidate = normalized(value);
  const root = normalized(REPO_ROOT);
  return candidate === root || candidate.startsWith(`${root}/`) || candidate.includes(`${root}/`);
}

export function ownsProcess(processInfo, identity) {
  const executable = normalized(processInfo.executablePath);
  const command = normalized(processInfo.commandLine);
  const combined = `${executable}\n${command}`;
  if (!underRoot(executable) && !underRoot(command)) return false;
  if (combined.includes("/scripts/run-dev.command")
    || combined.includes("/scripts/run-dev.ps1")
    || combined.includes("/scripts/run-built.command")
    || combined.includes("/scripts/run-built.ps1")
    || combined.includes("/scripts/rebuild.command")
    || combined.includes("/scripts/rebuild.ps1")) {
    return true;
  }

  const executableName = identity.executable.toLowerCase();
  const macBundleExecutable = `/${identity.label.toLowerCase()}.app/contents/macos/${executableName}`;
  const windowsExecutable = `/${executableName}.exe`;
  if (identity.kind === "electron") {
    return combined.includes("/node_modules/.bin/electron-vite")
      || combined.includes("/node_modules/electron-vite/")
      || combined.includes("/node_modules/electron/dist/")
      || combined.includes(macBundleExecutable)
      || combined.includes(windowsExecutable);
  }
  if (identity.kind === "tauri") {
    return combined.includes("/node_modules/.bin/tauri")
      || combined.includes("/node_modules/@tauri-apps/cli/")
      || combined.includes("/node_modules/.bin/vite")
      || combined.includes("/node_modules/vite/")
      || combined.includes(`/target/debug/${executableName}`)
      || combined.includes(`/target/release/${executableName}`)
      || combined.includes(macBundleExecutable);
  }
  if (identity.kind === "web") {
    return combined.includes("/node_modules/.bin/vite")
      || combined.includes("/node_modules/.bin/tsx")
      || combined.includes("/node_modules/.bin/concurrently")
      || combined.includes("/node_modules/vite/")
      || combined.includes("/node_modules/tsx/")
      || combined.includes("/node_modules/concurrently/")
      || combined.includes("/scripts/run-dev.ts");
  }
  throw new Error(`Unknown runtime identity: ${identity.kind}`);
}

function parsePosixProcesses(stdout) {
  return stdout.trim().split("\n").filter(Boolean).map((line) => {
    const match = /^\s*(\d+)\s+(\d+)\s+(.*)$/.exec(line);
    if (!match) return undefined;
    return {
      pid: Number(match[1]),
      parentPid: Number(match[2]),
      executablePath: "",
      commandLine: match[3],
    };
  }).filter(Boolean);
}

async function listProcesses() {
  if (process.platform !== "win32") {
    const { stdout } = await execFileAsync("ps", ["-axo", "pid=,ppid=,command="], {
      encoding: "utf8",
      timeout: 5000,
      maxBuffer: 8 * 1024 * 1024,
    });
    return parsePosixProcesses(stdout);
  }

  const script = [
    "Get-CimInstance Win32_Process |",
    "Select-Object ProcessId,ParentProcessId,ExecutablePath,CommandLine |",
    "ConvertTo-Json -Compress",
  ].join(" ");
  const { stdout } = await execFileAsync("powershell.exe", [
    "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script,
  ], {
    encoding: "utf8",
    timeout: 10000,
    maxBuffer: 16 * 1024 * 1024,
  });
  const decoded = JSON.parse(stdout || "[]");
  const rows = Array.isArray(decoded) ? decoded : [decoded];
  return rows.map((row) => ({
    pid: Number(row.ProcessId),
    parentPid: Number(row.ParentProcessId),
    executablePath: row.ExecutablePath ?? "",
    commandLine: row.CommandLine ?? "",
  }));
}

function descendantsOf(processes, roots) {
  const selected = new Set(roots);
  let changed = true;
  while (changed) {
    changed = false;
    for (const item of processes) {
      if (selected.has(item.parentPid) && !selected.has(item.pid)) {
        selected.add(item.pid);
        changed = true;
      }
    }
  }
  return selected;
}

function ancestorsOf(processes, pid) {
  const byPid = new Map(processes.map((item) => [item.pid, item]));
  const selected = new Set([pid]);
  let current = byPid.get(pid)?.parentPid;
  while (current && !selected.has(current)) {
    selected.add(current);
    current = byPid.get(current)?.parentPid;
  }
  return selected;
}

const delay = (milliseconds) => new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));

export async function claimRuntime(token) {
  await writeFile(OWNER_FILE, `${token}\n`, { encoding: "utf8", mode: 0o600 });
}

export async function isRuntimeOwner(token) {
  try {
    return (await readFile(OWNER_FILE, "utf8")).trim() === token;
  } catch (error) {
    if (error?.code === "ENOENT") return false;
    throw error;
  }
}

export async function releaseRuntime(token) {
  if (!await isRuntimeOwner(token)) return false;
  try {
    await unlink(OWNER_FILE);
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  return true;
}

async function stopRuntime(identity) {
  const initial = await listProcesses();
  const excluded = ancestorsOf(initial, process.pid);
  const ownedRoots = initial.filter((item) => !excluded.has(item.pid) && ownsProcess(item, identity)).map((item) => item.pid);
  if (ownedRoots.length === 0) return;

  const initialTargets = descendantsOf(initial, ownedRoots);
  process.stdout.write(`Stopping the existing ${identity.label} runtime (pid ${ownedRoots.join(", ")}).\n`);

  if (process.platform === "win32") {
    for (const pid of ownedRoots) {
      try {
        await execFileAsync("taskkill.exe", ["/PID", String(pid), "/T"], { timeout: 5000 });
      } catch {
        // A process may exit while another root tears down the same tree.
      }
    }
  } else {
    for (const pid of initialTargets) {
      try { process.kill(pid, "SIGTERM"); } catch {}
    }
  }

  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    const current = await listProcesses();
    if (!current.some((item) => initialTargets.has(item.pid))) return;
    await delay(100);
  }

  const current = await listProcesses();
  const currentOwnedRoots = current
    .filter((item) => initialTargets.has(item.pid) && !excluded.has(item.pid) && ownsProcess(item, identity))
    .map((item) => item.pid);
  const forceTargets = descendantsOf(current, currentOwnedRoots);
  if (process.platform === "win32") {
    for (const pid of currentOwnedRoots) {
      try {
        await execFileAsync("taskkill.exe", ["/PID", String(pid), "/T", "/F"], { timeout: 5000 });
      } catch {}
    }
  } else {
    for (const pid of forceTargets) {
      try { process.kill(pid, "SIGKILL"); } catch {}
    }
  }
}

async function assertEndpointAvailable(host, port) {
  await new Promise((resolveProbe, rejectProbe) => {
    const probe = createServer();
    probe.unref();
    probe.once("error", (error) => {
      rejectProbe(new Error(
        `Cannot bind ${host}:${port}: ${error.code ?? error.message}. The listener was left running because its app identity is unknown.`,
      ));
    });
    probe.listen({ host, port, exclusive: true }, () => {
      probe.close((error) => error ? rejectProbe(error) : resolveProbe());
    });
  });
}

async function waitForHttp(url, timeoutMilliseconds) {
  const deadline = Date.now() + timeoutMilliseconds;
  let lastError = "no response";
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(1000) });
      if (response.ok) return;
      lastError = `HTTP ${response.status}`;
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
    await delay(200);
  }
  throw new Error(`${url} did not become ready within ${timeoutMilliseconds} ms (${lastError}).`);
}

async function waitForExecutable(executablePath, timeoutMilliseconds) {
  const expected = normalized(resolve(executablePath));
  const repoRelativeExpected = normalized(relative(REPO_ROOT, expected));
  const targetMarker = expected.lastIndexOf("/target/");
  const targetRelativeExpected = targetMarker >= 0 ? expected.slice(targetMarker + 1) : "";
  const deadline = Date.now() + timeoutMilliseconds;
  while (Date.now() < deadline) {
    const processes = await listProcesses();
    const byPid = new Map(processes.map((item) => [item.pid, item]));
    const found = processes.some((item) => {
      const executable = normalized(item.executablePath);
      const command = normalized(item.commandLine);
      if (executable === expected || command === expected || command.startsWith(expected + " ")) return true;
      const relativeMatch = command === repoRelativeExpected
        || command.startsWith(repoRelativeExpected + " ")
        || (targetRelativeExpected && (
          command === targetRelativeExpected || command.startsWith(targetRelativeExpected + " ")
        ));
      if (!relativeMatch) return false;
      let parent = byPid.get(item.parentPid);
      while (parent) {
        if (underRoot(parent.executablePath) || underRoot(parent.commandLine)) return true;
        parent = byPid.get(parent.parentPid);
      }
      return false;
    });
    if (found) return;
    await delay(100);
  }
  throw new Error(`${executablePath} did not start within ${timeoutMilliseconds} ms.`);
}

function parseIdentity(args) {
  const kind = args.shift();
  const label = args.shift();
  const executable = args.shift();
  if (!["electron", "tauri", "web"].includes(kind) || !label || !executable) {
    throw new Error("Expected runtime identity: <electron|tauri|web> <label> <executable-name>.");
  }
  return { kind, label, executable };
}

async function main(args) {
  const command = args.shift();
  if (command === "claim") {
    const token = args.shift();
    if (!token) throw new Error("Expected claim <token>.");
    await claimRuntime(token);
    return;
  }
  if (command === "is-owner") {
    const token = args.shift();
    if (!token) throw new Error("Expected is-owner <token>.");
    if (!await isRuntimeOwner(token)) process.exitCode = 3;
    return;
  }
  if (command === "release-if-owner") {
    const token = args.shift();
    if (!token) throw new Error("Expected release-if-owner <token>.");
    await releaseRuntime(token);
    return;
  }
  if (command === "stop") {
    await stopRuntime(parseIdentity(args));
    return;
  }
  if (command === "stop-if-owner") {
    const token = args.shift();
    if (!token) throw new Error("Expected stop-if-owner <token> <identity...>.");
    const identity = parseIdentity(args);
    if (!await isRuntimeOwner(token)) return;
    await stopRuntime(identity);
    await releaseRuntime(token);
    return;
  }
  if (command === "check-endpoint") {
    const host = args.shift();
    const port = Number(args.shift());
    if (!host || !Number.isInteger(port)) throw new Error("Expected check-endpoint <host> <port>.");
    await assertEndpointAvailable(host, port);
    return;
  }
  if (command === "wait-http") {
    const url = args.shift();
    const timeout = Number(args.shift() ?? "60000");
    if (!url || !Number.isFinite(timeout)) throw new Error("Expected wait-http <url> [timeout-ms].");
    await waitForHttp(url, timeout);
    return;
  }
  if (command === "wait-process") {
    const executablePath = args.shift();
    const timeout = Number(args.shift() ?? "30000");
    if (!executablePath || !Number.isFinite(timeout)) throw new Error("Expected wait-process <executable-path> [timeout-ms].");
    await waitForExecutable(executablePath, timeout);
    return;
  }
  throw new Error("Expected one of: claim, is-owner, release-if-owner, stop, stop-if-owner, check-endpoint, wait-http, wait-process.");
}

const invokedPath = process.argv[1] ? resolve(process.argv[1]) : "";
if (invokedPath === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  });
}
