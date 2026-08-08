// Frontend logging. The sandboxed webview never opens a log file itself; it
// builds the JSON-Lines envelope and forwards each structured event to the Rust
// core (the `log_event` command), which owns the per-session file and applies
// redaction + the debug gate authoritatively (see src-tauri/src/logging.rs). If
// forwarding fails, this degrades to the console and never throws — logging
// must never break the app.
//
// Levels: error / warn / info / debug. `debug` is developer-only — emitted only
// in a Vite dev build or when the core reports ONECOPY_DEBUG=1 — so the
// per-frame / per-keystroke firehose costs nothing on an end user's machine.

import { invoke } from "@tauri-apps/api/core";

export type LogFields = Record<string, unknown>;

type Level = "debug" | "info" | "warn" | "error";

// The debug gate. `debug` events are dropped (never written) unless this is a
// Vite dev build or the core reports ONECOPY_DEBUG=1. This is a RUNTIME check,
// not dead-code elimination: `import.meta.env.DEV` folds to a constant, but it is
// OR'd with the runtime `runtimeDebug`, so debug call sites are NOT removed in a
// production build — emit() returns at the gate, but the field-arg object is
// still built at the call site. Keep debug field args cheap.
//
// Two gates exist by design: this frontend gate avoids forwarding the
// per-frame/per-keystroke firehose over IPC, while the Rust core re-gates as the
// authoritative writer. `runtimeDebug` mirrors the core's gate and is fetched
// once in initLogging(); until that resolves (a brief startup window) a packaged
// ONECOPY_DEBUG=1 build falls back to the dev default, so the earliest frontend
// debug events in that mode may not be forwarded.
let runtimeDebug = false;

function debugEnabled(): boolean {
  return import.meta.env.DEV || runtimeDebug;
}

// Denied field names (exact, case-insensitive) — the non-destructive redaction
// backstop, mirroring the Rust writer's set. It replaces only matched values
// and never inspects or edits prose.
const DENIED_KEYS = new Set([
  "apikey",
  "authorization",
  "token",
  "password",
  "secret",
]);

// Recurse only into plain objects and arrays; pass every other object type
// (Date, Map, Set, class instances) through unchanged. Recursing them via
// Object.entries would silently flatten e.g. a Date to `{}`, so this guard keeps
// the redactor type-preserving — it can replace a denied value but never lose one.
function isPlainObject(value: object): boolean {
  const proto = Object.getPrototypeOf(value);
  return proto === Object.prototype || proto === null;
}

function redact(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(redact);
  }
  if (value !== null && typeof value === "object" && isPlainObject(value)) {
    const out: Record<string, unknown> = {};
    for (const [key, val] of Object.entries(value as Record<string, unknown>)) {
      out[key] = DENIED_KEYS.has(key.toLowerCase()) ? "[redacted]" : redact(val);
    }
    return out;
  }
  return value;
}

function emit(level: Level, message: string, fields?: LogFields): void {
  if (level === "debug" && !debugEnabled()) return;

  // `time` is stamped at the event instant (UTC ISO 8601 ms + Z). Fields are
  // spread first and the envelope keys last, so a field that happens to be named
  // `time` / `level` / `message` can never clobber the envelope, and redaction
  // (which runs only on the fields) can never touch the message.
  const entry: Record<string, unknown> = {
    ...(fields ? (redact(fields) as LogFields) : {}),
    time: new Date().toISOString(),
    level,
    message,
  };

  // Forward to the core (the authoritative writer, which re-applies redaction).
  // Fire-and-forget so logging never blocks the UI. On failure, degrade to the
  // console — never swallow, never throw. The invoke is deferred into a promise
  // chain so a synchronous throw (or a non-promise return) can never escape.
  void Promise.resolve()
    .then(() => invoke("log_event", { entry }))
    .catch((forwardError) => {
      const consoleFn =
        level === "error"
          ? console.error
          : level === "warn"
            ? console.warn
            : console.log;
      consoleFn(
        `[onecopy:log:${level}] ${message}`,
        entry,
        "(forward failed)",
        forwardError,
      );
    });
}

export const log = {
  debug: (message: string, fields?: LogFields) => emit("debug", message, fields),
  info: (message: string, fields?: LogFields) => emit("info", message, fields),
  warn: (message: string, fields?: LogFields) => emit("warn", message, fields),
  error: (message: string, fields?: LogFields) => emit("error", message, fields),
};

// Builds an `error` field with full fidelity — name, message, stack, and the
// cause chain — for any caught value. Tauri command rejections surface as plain
// strings, so non-Error values are preserved rather than flattened.
export function toErrorFields(error: unknown): LogFields {
  return { error: describeError(error) };
}

function describeError(error: unknown): unknown {
  if (error instanceof Error) {
    const described: Record<string, unknown> = {
      name: error.name,
      message: error.message,
    };
    if (error.stack) described.stack = error.stack;
    // `Error.cause` is ES2022; read it defensively so this does not depend on
    // the project's TS lib target. The Tauri webview supports it at runtime.
    const cause = (error as { cause?: unknown }).cause;
    if (cause !== undefined) described.cause = describeError(cause);
    return described;
  }
  if (error !== null && typeof error === "object") {
    return error;
  }
  return { message: String(error) };
}

// Fetches the core's debug gate once at startup, so a packaged build launched
// with ONECOPY_DEBUG=1 also emits frontend debug. Best-effort: on failure the
// import.meta.env.DEV default stands.
export async function initLogging(): Promise<void> {
  try {
    runtimeDebug = await invoke<boolean>("logging_debug_enabled");
  } catch (e) {
    log.warn("logging: could not read debug gate from core", toErrorFields(e));
  }
}
