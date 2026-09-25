// Threading discipline for the Tauri command registry (see the THREADING
// comment above `dispatch()` in `src-tauri/src/lib.rs`).
//
// A plain `#[tauri::command]` fn runs on Tauri's MAIN thread; an `async fn`
// command runs on a tokio worker. Neither may block on SQLite, the
// filesystem, or a subprocess directly — a plain command would freeze every
// window, and an async command would starve every other command's dispatch
// (see the responsiveness audit's W-H2/C-H2 findings). The one place either
// may touch those is through `dispatch()`, which moves the work onto
// `tauri::async_runtime::spawn_blocking`'s dedicated pool.
//
// This is a source-text check: it needs no running backend, so it cannot
// drift from the shipped file the way a hand-kept inventory would, and it
// catches the regression class directly — a new command added straight to
// the index/filesystem/subprocess boundary without going through dispatch().

import { readFileSync } from "node:fs";
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

// Direct crossings of the index/filesystem/subprocess boundary. Almost every
// command in this file that needs one resolves it through `paths::data_root`
// or `index_store::open` directly in its own body (see the many `let
// data_root = paths::data_root(&app)?;` lines) — the pattern this codebase
// already uses everywhere — so this short, direct list catches real crossings
// without following every helper function's own call graph, which would
// make the check as fragile as the code it is meant to guard.
const FORBIDDEN_BOUNDARY = [
  /paths::data_root\(/,
  /index_store::open\(/,
  /std::fs::/,
  /subprocess::/,
];

const DISPATCH_MARKERS = [/\bdispatch\(/, /spawn_blocking\(/];

describe("command threading", () => {
  const commands = findCommands(rustLib);

  it("finds something to check", () => {
    // If this reads zero, the header/brace parse broke and every assertion
    // below would pass vacuously.
    expect(commands.length).toBeGreaterThan(30);
  });

  it("keeps plain commands off the index, filesystem, and subprocess, and routes async commands' access through dispatch()", () => {
    const violations: string[] = [];
    for (const command of commands) {
      const touchesBoundary = FORBIDDEN_BOUNDARY.some((pattern) => pattern.test(command.body));
      if (!touchesBoundary) continue;
      if (!command.isAsync) {
        violations.push(
          `${command.name}: a plain #[tauri::command] touches the index, filesystem, or a subprocess directly — it runs on the MAIN thread and would freeze every window`,
        );
        continue;
      }
      const dispatched = DISPATCH_MARKERS.some((pattern) => pattern.test(command.body));
      if (!dispatched) {
        violations.push(
          `${command.name}: an async command touches the index, filesystem, or a subprocess without dispatch() — it would block a tokio worker`,
        );
      }
    }
    expect(violations).toEqual([]);
  });
});
