// The IPC contract between the webview and the Rust command registry.
//
// A drift here compiles clean on BOTH sides — TypeScript does not know the
// command names, and Rust does not know its callers — and surfaces at runtime
// as a rejected promise that most call sites swallow into a log line. That is
// exactly the "this button does nothing" class of bug, and nothing could catch
// the next one.
//
// The command checks read source text, deliberately: they need no running
// backend and no mock, so they cannot drift from the shipped files the way a
// hand-kept mirror would. The names the core sends inside events and payloads
// are checked against the shared fixture the Rust suite also checks.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { identityFromKey, keyOf } from "../../src/models/items";
import { workReasonKey } from "../../src/models/workReasons";
import { BACKGROUND_CLASS_IDS, CORE_CLASS_STATES } from "../../src/state/derived-work-store";
import { CORE_NAMES } from "../helpers/coreNames";
import { argumentProblems, commandParameters, invokeCallsIn } from "../helpers/ipcContract";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function sourceFiles(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) sourceFiles(full, out);
    else if (/\.tsx?$/.test(entry)) out.push(full);
  }
  return out;
}

const frontendCalls = sourceFiles(join(repoRoot, "src"))
  .flatMap((file) => invokeCallsIn(readFileSync(file, "utf8")).map((call) => ({ file, ...call })));
const rustLib = readFileSync(join(repoRoot, "src-tauri/src/lib.rs"), "utf8");
const invoked = new Set(frontendCalls.map((call) => call.command));

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
    const missing = [...invoked].filter((name) => !registered.has(name ?? ""));
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
  it("checks every call against required and optional native parameters", () => {
    const commands = commandParameters(rustLib);
    expect([...registered].filter((command) => !commands.has(command))).toEqual([]);
    const problems = frontendCalls.flatMap((call) => {
      if (call.command === null) return [`${call.file}: dynamic command`];
      const parameters = commands.get(call.command);
      if (parameters === undefined) return []; // Registry coverage owns absent commands.
      return argumentProblems(call, parameters).map((problem) => `${call.file}: ${call.command}: ${problem}`);
    });
    expect(problems).toEqual([]);
  });

  it("reads shorthand and quoted keys without treating nested values or comments as arguments", () => {
    const calls = invokeCallsIn(`
      // invoke("not_a_call", { wrong: true });
      invoke<Result>("save", { renamed, "reportFailure": false,
        payload: values.map(value => ({ nested: value, punctuation: "},:" })),
        ...{ literalSpread: true } });
    `);
    expect(calls).toEqual([{ command: "save", keys: ["renamed", "reportFailure", "payload", "literalSpread"], unresolved: [] }]);
    expect(argumentProblems(calls[0]!, [{ key: "changes", required: true },
      ...["reportFailure", "payload", "literalSpread"].map(key => ({ key, required: false }))]))
      .toEqual(["unknown renamed", "missing changes"]);
  });

  it("reports dynamic arguments and computed keys rather than silently skipping them", () => {
    const calls = invokeCallsIn(`invoke("save", args); invoke("save", { ...args });
      invoke("save", { [key]: value }); invoke(command, {});`);
    expect(calls.map(call => call.unresolved)).toEqual([["nonliteral arguments"], ["nonliteral arguments"], ["computed argument key"], []]);
    expect(calls[3]!.command).toBeNull();
  });

  it("recognizes import aliases and calls with no argument object", () => {
    const calls = invokeCallsIn(`import { invoke as native } from "@tauri-apps/api/core";
      native("save");`);
    expect(argumentProblems(calls[0]!, [{ key: "changes", required: true }])).toEqual(["missing changes"]);
  });

  it("excludes injected native types and permits omitted Option parameters, including defaulted ones", () => {
    const commands = commandParameters(`
      #[tauri::command]
      async fn save(readiness: tauri::State<'_, Gate>, app: AppHandle,
        changes: serde_json::Map<String, Value>, report_failure: Option<bool>) -> Result<(), String> { todo!() }
      #[tauri::command]
      fn notify(window: tauri::WebviewWindow, operation_id: u64) {}
    `);
    expect(commands.get("save")).toEqual([{ key: "changes", required: true }, { key: "reportFailure", required: false }]);
    expect(commands.get("notify")).toEqual([{ key: "operationId", required: true }]);
    expect(argumentProblems(invokeCallsIn('invoke("save", { changes });')[0]!, commands.get("save")!)).toEqual([]);
  });
});

describe("names the core sends and the frontend keys on", () => {
  it("spells and parses item keys as the core does", () => {
    for (const { hash, pathId, key } of CORE_NAMES.itemKeys) {
      expect(keyOf(hash, pathId)).toBe(key);
      expect(identityFromKey(key)).toEqual(hash === null ? { hash: null, pathId } : { hash, pathId: 0 });
    }
  });

  it("knows exactly the background-work classes and row states the core reports", () => {
    expect([...BACKGROUND_CLASS_IDS]).toEqual(CORE_NAMES.backgroundWork.classIds);
    expect([...CORE_CLASS_STATES].sort()).toEqual([...CORE_NAMES.backgroundWork.classStates].sort());
  });

  it("names every reason the core gives for held-back work", () => {
    const unnamed = CORE_NAMES.backgroundWork.reasons.filter((reason) => workReasonKey(reason) === null);
    expect(unnamed).toEqual([]);
  });
});
