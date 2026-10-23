// Threading discipline for the Tauri command registry.
//
// A plain `#[tauri::command]` fn runs on Tauri's MAIN thread; an `async fn`
// command runs on a tokio worker. Neither may block on SQLite, the
// filesystem, a subprocess, or a lock retained through those effects: a plain
// command would freeze every window, and an async command would starve every
// other command's dispatch. The one place either may touch those is through
// `dispatch()`, which moves the work onto `tauri::async_runtime::spawn_blocking`'s
// dedicated pool.
//
// This scans the shipped Rust source rather than a running backend: the
// commands and their delegate call graph are derived from the source, so a new
// command added straight to the index/filesystem/subprocess boundary without
// going through dispatch() fails here, and the planted cases below prove the
// scan can fail.

import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const rustLib = readFileSync(
  fileURLToPath(new URL("../../src-tauri/src/lib.rs", import.meta.url)),
  "utf8",
);

interface CommandFn {
  name: string;
  isAsync: boolean;
  body: string;
}

/**
 * Every `#[tauri::command...]`-attributed fn in `lib.rs`, with its body
 * extracted by balanced braces (a naive `[\s\S]*?\}` would stop at the first
 * nested `}`, of which every one of these bodies has several).
 */
function findCommands(source: string): CommandFn[] {
  const commands: CommandFn[] = [];
  const header = /#\[tauri::command(?:\([^)]*\))?\]\s*(?:#\[[^\]]*\]\s*)*(async\s+)?fn\s+([a-zA-Z_]+)\s*\(/g;
  let match: RegExpExecArray | null;
  while ((match = header.exec(source))) {
    const isAsync = Boolean(match[1]);
    const name = match[2]!;
    // Walk past the balanced argument-list parens to the return type/body.
    let i = header.lastIndex - 1; // sitting on the opening '(' of the args
    let parenDepth = 0;
    do {
      if (source[i] === "(") parenDepth++;
      else if (source[i] === ")") parenDepth--;
      i++;
    } while (parenDepth > 0 && i < source.length);
    while (source[i] !== "{" && i < source.length) i++;
    const bodyStart = i;
    let braceDepth = 0;
    do {
      if (source[i] === "{") braceDepth++;
      else if (source[i] === "}") braceDepth--;
      i++;
    } while (braceDepth > 0 && i < source.length);
    commands.push({ name, isAsync, body: source.slice(bodyStart, i) });
  }
  return commands;
}

// Direct crossings of the index/filesystem/subprocess boundary. The command
// bodies in this file are thin delegates to their owning modules, so the
// async rule below is structural (every async command dispatches) and this
// short list guards plain commands against reaching for the boundary
// themselves.
const FORBIDDEN_BOUNDARY = [
  /paths::data_root\(/,
  /index_store::open\(/,
  /std::fs::/,
  /subprocess::(?!signal_all_running_for_exit\b)/,
  // This delegate waits for ACTIVE, disposes SQLite, and removes its file.
  /viewer_sequence::close\(/,
  // This writer can be reached while a control reports failed delivery.
  /activity::record(?:_work)?\(/,
];

// Async commands that await their own non-blocking work instead of running a
// blocking body; everything else async goes through dispatch(). The standalone
// quit question awaits a dialog that rfd runs on a thread of its own. The
// folder picker awaits its native completion callback; AppKit sheet setup is
// queued onto the main thread and never blocks a runtime worker.
const ASYNC_WITHOUT_DISPATCH = new Set(["quit_question", "pick_directories"]);

const DISPATCH_MARKERS = [/\bdispatch\(/, /spawn_blocking\(/];

// Thin delegates can hide both disk access and a lock retained by its owner
// through disk access. Derive this graph from the real source modules so a
// newly introduced helper cannot make a main-thread crossing disappear.
const rustDirectory = new URL("../../src-tauri/src/", import.meta.url);
const rustSources = new Map(
  readdirSync(rustDirectory)
    .filter((name) => name.endsWith(".rs"))
    .map((name) => [name.slice(0, -3), readFileSync(new URL(name, rustDirectory), "utf8")]),
);

function rustCode(source: string): string {
  // Keep offsets while excluding comments and literals from the call graph.
  return source.replace(/\/\/[^\n]*|\/\*[\s\S]*?\*\/|"(?:\\[\s\S]|[^"\\])*"|'(?:\\.|[^'\\\n])'/g,
    (literal) => literal.replace(/[^\n]/g, " "));
}

function findFunctions(source: string): Array<{ name: string; body: string }> {
  const code = rustCode(source);
  const header = /\bfn\s+([A-Za-z_]\w*)\s*(?:<[^{};]*?>)?\s*\(/g;
  const functions: Array<{ name: string; body: string }> = [];
  let depth = 0;
  let scanned = 0;
  let match: RegExpExecArray | null;
  while ((match = header.exec(code))) {
    while (scanned < match.index) {
      if (code[scanned] === "{") depth++;
      else if (code[scanned] === "}") depth--;
      scanned++;
    }
    // Method calls do not resolve as module::function; only free functions
    // form the command delegate graph.
    if (depth !== 0) continue;
    let i = header.lastIndex - 1;
    let argumentsDepth = 0;
    do {
      if (code[i] === "(") argumentsDepth++;
      else if (code[i] === ")") argumentsDepth--;
      i++;
    } while (argumentsDepth > 0 && i < code.length);
    while (code[i] !== "{" && i < code.length) i++;
    const bodyStart = i;
    let bodyDepth = 0;
    do {
      if (code[i] === "{") bodyDepth++;
      else if (code[i] === "}") bodyDepth--;
      i++;
    } while (bodyDepth > 0 && i < code.length);
    functions.push({ name: match[1]!, body: code.slice(bodyStart, i) });
    scanned = i;
    header.lastIndex = i;
  }
  return functions;
}

function synchronousBody(body: string): string {
  // These adapters execute their arguments on another owner. A delegate's
  // spawned callback is not work on the command caller's thread.
  const offThread = /\b(?:dispatch|spawn_blocking|spawn_thread)\s*\(|\b(?:std::thread|tokio|tauri::async_runtime)::spawn\s*\(|\.spawn\s*\(/g;
  let match: RegExpExecArray | null;
  while ((match = offThread.exec(body))) {
    let i = offThread.lastIndex - 1;
    let depth = 0;
    do {
      if (body[i] === "(") depth++;
      else if (body[i] === ")") depth--;
      i++;
    } while (depth > 0 && i < body.length);
    body = body.slice(0, match.index) + " ".repeat(i - match.index) + body.slice(i);
    offThread.lastIndex = i;
  }
  return body;
}

function commandBoundaryPaths(sources: ReadonlyMap<string, string>): Map<string, string[]> {
  const nodes = new Map<string, { calls: string[]; boundary: boolean }>();
  for (const [module, source] of sources) {
    const code = rustCode(source);
    const imports = new Map<string, string>();
    for (const use of code.matchAll(/\buse\s+crate::(\w+)::(?:\{([^}]+)\}|(\w+))\s*;/g)) {
      for (const name of (use[2] ?? use[3]!).split(",")) {
        const imported = name.trim().match(/^(\w+)(?:\s+as\s+(\w+))?$/);
        if (imported) imports.set(imported[2] ?? imported[1]!, `${use[1]}::${imported[1]}`);
      }
    }
    for (const fn of findFunctions(source)) {
      const body = synchronousBody(fn.body);
      const calls = Array.from(body.matchAll(/(?<![\w:.])((?:\w+::)*\w+)\s*\(/g), (call) => {
        const name = call[1]!.replace(/^crate::/, "");
        return name.includes("::") ? name : imports.get(name) ?? `${module}::${name}`;
      });
      nodes.set(`${module}::${fn.name}`, {
        calls,
        boundary: FORBIDDEN_BOUNDARY.some((pattern) => pattern.test(body))
          // RUNTIME is retained while signaling native cancellation. The
          // transcription registry can be held by publish_if_active through
          // SQLite write/commit, so even a runtime read can wait behind disk.
          || (module === "derived_runtime" && /\bRUNTIME\s*\.\s*0\s*\.\s*lock\s*\(/.test(body)),
      });
    }
  }
  const paths = new Map<string, string[]>();
  for (const name of findCommands(sources.get("lib")!).map((command) => command.name)) {
    const pending = [[`lib::${name}`]];
    const visited = new Set<string>();
    while (pending.length) {
      const path = pending.shift()!;
      const current = path.at(-1)!;
      if (visited.has(current)) continue;
      visited.add(current);
      const node = nodes.get(current);
      if (!node) continue;
      if (node.boundary) {
        paths.set(name, path);
        break;
      }
      for (const call of node.calls) pending.push([...path, call]);
    }
  }
  return paths;
}

function boundaryViolations(commands: CommandFn[], sources = rustSources): string[] {
  const delegatePaths = commandBoundaryPaths(sources);
  const violations: string[] = [];
  for (const command of commands) {
    const touchesBoundary = FORBIDDEN_BOUNDARY.some((pattern) => pattern.test(synchronousBody(rustCode(command.body))))
      || delegatePaths.has(command.name);
    if (!touchesBoundary) continue;
    const path = delegatePaths.get(command.name)?.join(" -> ") ?? command.name;
    if (!command.isAsync) {
      violations.push(
        `${command.name}: a plain #[tauri::command] reaches blocking work (${path}) — it runs on the MAIN thread and would freeze every window`,
      );
      continue;
    }
    // The graph already excludes work passed through a blocking owner. Any
    // remaining path is undispatched even if another part of the command
    // happens to call dispatch().
    violations.push(
      `${command.name}: an async command reaches blocking work (${path}) outside dispatch() — it would block a tokio worker`,
    );
  }
  return violations;
}

describe("command threading", () => {
  const commands = findCommands(rustLib);

  it("finds something to check", () => {
    // If this reads zero, the header/brace parse broke and every assertion
    // below would pass vacuously.
    expect(commands.length).toBeGreaterThan(30);
  });

  it("routes command delegates to blocking work through dispatch()", () => {
    expect(boundaryViolations(commands)).toEqual([]);
  });

  it("rejects a planted undispatched viewer-sequence close delegate", () => {
    const planted = findCommands(`
      #[tauri::command]
      fn viewer_sequence_close(token: Option<String>) -> Result<(), String> {
        viewer_sequence::close(token.as_deref())
      }
    `);
    expect(boundaryViolations(planted)).toHaveLength(1);
    expect(boundaryViolations(planted)[0]).toContain("viewer_sequence_close");
  });

  it.each(["", "async "])("rejects a planted %shidden native cancellation delegate", (mode) => {
    const source = `
      #[tauri::command]
      ${mode}fn planted_cancel() -> bool {
        hidden_control()
      }
      fn hidden_control() -> bool {
        derived_runtime::cancel_active_transcription()
      }
    `;
    const sources = new Map(rustSources).set("lib", source);
    const violations = boundaryViolations(findCommands(source), sources);
    expect(violations).toHaveLength(1);
    expect(violations[0]).toContain("lib::planted_cancel -> lib::hidden_control -> derived_runtime::cancel_active_transcription");
  });

  it("allows a delegate that moves its native work onto a worker", () => {
    const source = `
      #[tauri::command]
      fn worker_control() {
        std::thread::spawn(|| {
          hidden_control();
          std::fs::write("output", "done").unwrap();
        });
      }
      fn hidden_control() {
        derived_runtime::cancel_active_transcription();
      }
    `;
    const sources = new Map(rustSources).set("lib", source);
    expect(boundaryViolations(findCommands(source), sources)).toEqual([]);
  });

  it("rejects a native delegate outside a command's other dispatched work", () => {
    const source = `
      #[tauri::command]
      async fn partly_dispatched() -> bool {
        dispatch(|| Ok(())).await.unwrap();
        derived_runtime::cancel_active_transcription()
      }
    `;
    const sources = new Map(rustSources).set("lib", source);
    expect(boundaryViolations(findCommands(source), sources)).toHaveLength(1);
  });

  it("excuses only async commands that exist and really skip dispatch()", () => {
    const stale = [...ASYNC_WITHOUT_DISPATCH].filter((name) => {
      const command = commands.find((candidate) => candidate.name === name);
      return command === undefined
        || !command.isAsync
        || DISPATCH_MARKERS.some((pattern) => pattern.test(command.body));
    });
    expect(stale).toEqual([]);
  });

  it("runs every async command's body through dispatch()", () => {
    const undispatched = commands
      .filter((command) => command.isAsync && !ASYNC_WITHOUT_DISPATCH.has(command.name))
      .filter((command) => !DISPATCH_MARKERS.some((pattern) => pattern.test(command.body)))
      .map((command) => command.name);
    expect(undispatched).toEqual([]);
  });
});
