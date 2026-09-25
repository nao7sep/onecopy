// The IPC contract between the webview and the Rust command registry.
//
// A drift here compiles clean on BOTH sides — TypeScript does not know the
// command names, and Rust does not know its callers — and surfaces at runtime
// as a rejected promise that most call sites swallow into a log line. That is
// exactly the "this button does nothing" class of bug, and nothing could catch
// the next one.
//
// This is a source-text check, deliberately: it needs no running backend and
// no mock, so it cannot drift from the shipped files the way a hand-kept
// mirror would.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function sourceFiles(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) sourceFiles(full, out);
    else if (/\.tsx?$/.test(entry)) out.push(full);
  }
  return out;
}

const frontendSource = sourceFiles(join(repoRoot, "src"))
  .map((f) => readFileSync(f, "utf8"))
  .join("\n");

const rustLib = readFileSync(join(repoRoot, "src-tauri/src/lib.rs"), "utf8");

/** Command names the frontend actually calls. */
const invoked = new Set(
  [...frontendSource.matchAll(/invoke[^("]*\(\s*"([a-z_]+)"/g)].map(
    (m) => m[1]!,
  ),
);

/** Command names the Rust side registers in generate_handler!. */
const registered = new Set(
  (() => {
    const block = /generate_handler!\s*\[([\s\S]*?)\]/.exec(rustLib)?.[1] ?? "";
    return block
      .split(",")
      .map((s) => s.trim())
      .filter((s) => /^[a-z_]+$/.test(s));
  })(),
);

describe("the command registry", () => {
  it("registers something", () => {
    // If this ever reads zero the parse broke and every assertion below would
    // pass vacuously.
    expect(registered.size).toBeGreaterThan(10);
    expect(invoked.size).toBeGreaterThan(10);
  });

  it("has a Rust command behind every invoke the frontend makes", () => {
    const missing = [...invoked].filter((name) => !registered.has(name));
    expect(missing, "invoked but not registered").toEqual([]);
  });

  it("has no registered command the frontend never calls", () => {
    // Not a correctness failure, but dead surface is worth seeing: it is
    // usually a rename that left the old name behind.
    const unused = [...registered].filter((name) => !invoked.has(name));
    expect(unused, "registered but never invoked").toEqual([]);
  });
});

describe("command arguments", () => {
  /** `fn name(a: T, b_c: U)` → the camelCase keys Tauri expects. */
  function paramsOf(command: string): string[] | null {
    const pattern = new RegExp(
      `#\\[tauri::command\\][\\s\\S]{0,80}?fn\\s+${command}\\s*\\(([\\s\\S]*?)\\)\\s*->`,
    );
    const raw = pattern.exec(rustLib)?.[1];
    if (raw === undefined) return null;
    return raw
      .split(",")
      .map((p) => p.trim())
      .filter((p) => p.length > 0)
      .map((p) => p.split(":")[0]!.trim())
      // AppHandle/State are injected by Tauri, never sent by the caller.
      .filter((name) => !/^(app|window|state)$/.test(name))
      .map((name) => name.replace(/_([a-z])/g, (_m, c: string) => c.toUpperCase()));
  }

  /**
   * The invoke call's object-literal argument, matched to its BALANCED
   * closing brace rather than the first `}` — a payload built from a
   * `.map()` callback or a nested return value has closing braces of its
   * own before the argument's real end.
   */
  function objectArgOf(source: string, openBraceIndex: number): string {
    let depth = 0;
    let i = openBraceIndex;
    for (; i < source.length; i++) {
      if (source[i] === "{") depth++;
      else if (source[i] === "}") {
        depth--;
        if (depth === 0) break;
      }
    }
    return source.slice(openBraceIndex + 1, i);
  }

  /** Reserved words that can precede a `:` without being an object key —
   * a ternary's `cond ? null : value` reads exactly like `null:` to a
   * text scan. */
  const RESERVED_VALUE_WORDS = new Set(["null", "true", "false", "undefined"]);

  /**
   * Top-level (brace/bracket/paren depth 0 within the object argument) keys
   * only — a nested `{ hash, pathId }` inside a `.map()` callback is part of
   * one array-typed argument's element shape, not a key of the command's own
   * argument object, and must not be flagged as one.
   */
  function topLevelSentKeys(body: string): string[] {
    const keys: string[] = [];
    let depth = 0;
    const re = /[{}[\]()]|(?:^|[,{\s])([a-zA-Z][a-zA-Z0-9]*)\s*:/g;
    let match: RegExpExecArray | null;
    while ((match = re.exec(body))) {
      const token = match[0];
      if (token === "{" || token === "[" || token === "(") {
        depth++;
        continue;
      }
      if (token === "}" || token === "]" || token === ")") {
        depth = Math.max(0, depth - 1);
        continue;
      }
      const key = match[1];
      if (depth === 0 && key && !RESERVED_VALUE_WORDS.has(key)) {
        keys.push(key);
      }
    }
    return keys;
  }

  it("accepts every argument key the frontend sends", () => {
    const calls: { command: string; body: string }[] = [];
    const callStart = /invoke[^("]*\(\s*"([a-z_]+)"\s*,\s*\{/g;
    let match: RegExpExecArray | null;
    while ((match = callStart.exec(frontendSource))) {
      const openBrace = match.index + match[0].length - 1;
      calls.push({
        command: match[1]!,
        body: objectArgOf(frontendSource, openBrace),
      });
    }
    expect(calls.length).toBeGreaterThan(5);

    const mismatches: string[] = [];
    for (const { command, body } of calls) {
      const accepted = paramsOf(command);
      if (accepted === null) continue; // registry test above owns this case
      for (const key of topLevelSentKeys(body)) {
        if (!accepted.includes(key)) {
          mismatches.push(`${command}: sends "${key}", accepts [${accepted}]`);
        }
      }
    }
    expect([...new Set(mismatches)]).toEqual([]);
  });
});
