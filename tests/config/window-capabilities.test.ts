// @vitest-environment happy-dom
// Every Tauri call the frontend makes must be granted to the windows that run
// it, and no capability entry may grant something nothing calls.
//
// This exists because capabilities are runtime data: a call compiles even when
// its permission is absent, and the failure is a runtime rejection on a machine
// nobody automated. Nothing here is a hand-kept table. Each side comes from its
// owner:
//
// - which command a call invokes: the real @tauri-apps packages, run against
//   their own IPC mock, so every exported function and window/webview method
//   reports the command it sends;
// - which calls the frontend makes: the imports and call sites in src;
// - what a permission grants: the ACL manifest tauri-build generates from the
//   Tauri crates and plugins, resolving sets such as `core:default`;
// - which windows exist: tauri.conf.json, `new WebviewWindow(` in src and
//   `WebviewWindowBuilder::new(` in src-tauri/src.
//
// Not covered: app commands sent through `invoke` from @tauri-apps/api/core
// (this app's build.rs declares no app manifest, so capabilities do not govern
// them), URL and path scopes (only whether a command is allowed at all), and
// which window reaches which module: every window loads the same bundle, so
// every call must be granted to every window.

import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";

// tests/setup.ts swaps these packages for suite-wide doubles; this file needs
// the real ones to learn which command each call sends.
vi.unmock("@tauri-apps/api/core");
vi.unmock("@tauri-apps/api/event");
vi.unmock("@tauri-apps/api/window");
vi.unmock("@tauri-apps/api/webview");
vi.unmock("@tauri-apps/api/webviewWindow");
vi.unmock("@tauri-apps/plugin-opener");

const SOURCE_FILES = readdirSync("src", { recursive: true, encoding: "utf8" })
  .filter((path) => /\.tsx?$/.test(path))
  .map((path) => ({ path: join("src", path), text: readFileSync(join("src", path), "utf8") }));
const SOURCES = SOURCE_FILES.map((file) => file.text);

// ---- What each library call sends -------------------------------------------

const { mockIPC, mockWindows } = await import("@tauri-apps/api/mocks");
let sent: string[] = [];
mockIPC((command) => {
  sent.push(command);
  return null;
});
mockWindows("main");

/** The commands a call sends before it first yields. Every Tauri wrapper calls
 * `invoke` synchronously, and reading only this window keeps a constructor's
 * later `tauri://created` emit out of the next probe's record. */
async function commandsOf(call: () => unknown): Promise<string[]> {
  sent = [];
  let result: unknown;
  try {
    result = call();
  } catch {
    // Missing arguments may throw after the command was already sent.
  }
  const commands = [...new Set(sent)].map((command) => command.replace(/^plugin:/, ""));
  await Promise.resolve(result).catch(() => undefined);
  return commands;
}

type Surface = {
  /** Exported functions, and `Class.static` / `new Class` entries. */
  functions: Map<string, string[]>;
  /** Methods of the handle this module's `getCurrent*` returns. */
  methods: Map<string, string[]>;
};

const MODULES: Record<string, Record<string, unknown>> = {
  "@tauri-apps/api/event": await import("@tauri-apps/api/event"),
  "@tauri-apps/api/window": await import("@tauri-apps/api/window"),
  "@tauri-apps/api/webview": await import("@tauri-apps/api/webview"),
  "@tauri-apps/api/webviewWindow": await import("@tauri-apps/api/webviewWindow"),
  "@tauri-apps/plugin-opener": await import("@tauri-apps/plugin-opener"),
};

function methodNames(handle: object): string[] {
  const names = new Set<string>();
  for (let proto = Object.getPrototypeOf(handle); proto && proto !== Object.prototype; proto = Object.getPrototypeOf(proto)) {
    for (const name of Object.getOwnPropertyNames(proto)) {
      if (name !== "constructor" && typeof (handle as Record<string, unknown>)[name] === "function") names.add(name);
    }
  }
  return [...names];
}

async function surfaceOf(module: Record<string, unknown>): Promise<Surface> {
  const functions = new Map<string, string[]>();
  const methods = new Map<string, string[]>();
  for (const [name, value] of Object.entries(module)) {
    if (typeof value !== "function") continue;
    if (/^[A-Z]/.test(name)) {
      const type = value as (new (...args: unknown[]) => unknown) & Record<string, unknown>;
      functions.set(`new ${name}`, await commandsOf(() => new type("probe", {})));
      for (const member of Object.getOwnPropertyNames(type)) {
        if (typeof type[member] === "function") {
          functions.set(`${name}.${member}`, await commandsOf(() => (type[member] as () => unknown)()));
        }
      }
      continue;
    }
    functions.set(name, await commandsOf(() => (value as () => unknown)()));
    if (/^getCurrent/.test(name)) {
      const handle = (value as () => Record<string, () => unknown>)();
      for (const method of methodNames(handle)) {
        methods.set(method, await commandsOf(() => handle[method]!()));
      }
    }
  }
  return { functions, methods };
}

const SURFACES = new Map<string, Surface>();
for (const [specifier, module] of Object.entries(MODULES)) SURFACES.set(specifier, await surfaceOf(module));

// ---- Which calls the frontend makes -----------------------------------------

type Call = { file: string; call: string; commands: string[] };

function withoutComments(text: string): string {
  return text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:"'`\\])\/\/.*$/gm, "$1");
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** An opening call paren, after any explicit type arguments (`listen<T>(`). */
const OPEN_CALL = "(?:<[^()]*?>)?\\(";

const IMPORT = /import\s+(type\s+)?\{([^}]*)\}\s+from\s+["'](@tauri-apps\/[^"']+)["']/g;
const RAW_PLUGIN_INVOKE = /invoke\s*(?:<[^>]*>)?\(\s*["'`]plugin:([\w-]+\|[\w-]+)["'`]/g;

function frontendCalls(files: Array<{ path: string; text: string }>): { calls: Call[]; unseen: string[] } {
  const calls: Call[] = [];
  const unseen: string[] = [];
  for (const { path, text } of files) {
    const code = withoutComments(text);
    const body = code.replace(IMPORT, "");
    for (const [, command] of body.matchAll(RAW_PLUGIN_INVOKE)) {
      calls.push({ file: path, call: `invoke(plugin:${command})`, commands: [command!] });
    }
    const imports = [...code.matchAll(IMPORT)].filter(([, , , specifier]) => specifier !== "@tauri-apps/api/core");
    if (imports.length === 0) continue;
    const before = calls.length;
    let importsValues = false;
    const methods = new Map<string, Set<string>>();
    for (const [, typeOnly, names, specifier] of imports) {
      const surface = SURFACES.get(specifier!);
      if (!surface) throw new Error(`${path} imports ${specifier}, which this test does not probe`);
      for (const [method, commands] of surface.methods) {
        const known = methods.get(method) ?? new Set<string>();
        for (const command of commands) known.add(command);
        methods.set(method, known);
      }
      for (const entry of names!.split(",")) {
        const match = entry.trim().match(/^(type\s+)?(\w+)(?:\s+as\s+(\w+))?$/);
        if (!match) continue;
        const [, typeEntry, imported, local = imported] = match;
        if (!typeOnly && !typeEntry) importsValues = true;
        const forms: Array<[string, RegExp]> = [
          [imported!, new RegExp(`(?<![\\w.])${escapeRegExp(local!)}${OPEN_CALL}`)],
          [`new ${imported}`, new RegExp(`\\bnew\\s+${escapeRegExp(local!)}${OPEN_CALL}`)],
        ];
        for (const [key] of surface.functions) {
          const member = key.startsWith(`${imported}.`) ? key.slice(imported!.length + 1) : null;
          if (member) forms.push([key, new RegExp(`(?<![\\w.])${escapeRegExp(local!)}\\.${escapeRegExp(member)}${OPEN_CALL}`)]);
        }
        for (const [key, pattern] of forms) {
          const commands = surface.functions.get(key);
          if (commands && pattern.test(body)) calls.push({ file: path, call: key, commands });
        }
      }
    }
    // A handle's type cannot be read from text, so a method name any imported
    // handle defines counts as a call of every such method.
    for (const [method, commands] of methods) {
      if (new RegExp(`\\.${escapeRegExp(method)}${OPEN_CALL}`).test(body)) {
        calls.push({ file: path, call: `.${method}()`, commands: [...commands] });
      }
    }
    // A file that imports a Tauri value but shows no call means the scan has
    // gone blind to how it calls.
    if (importsValues && calls.length === before) unseen.push(path);
  }
  return { calls, unseen };
}

const FRONTEND = frontendCalls(SOURCE_FILES);
const USED = new Set(FRONTEND.calls.flatMap((call) => call.commands));

// ---- What a permission grants -----------------------------------------------

const ACL_PATH = "src-tauri/gen/schemas/acl-manifests.json";

type AclPlugin = {
  default_permission?: { permissions: string[] } | null;
  permissions: Record<string, { commands: { allow: string[]; deny: string[] } }>;
  permission_sets: Record<string, { permissions: string[] }>;
};

function loadAcl(): Record<string, AclPlugin> {
  if (!existsSync(ACL_PATH)) {
    throw new Error(`${ACL_PATH} is missing; tauri-build writes it, so run \`cargo build --manifest-path src-tauri/Cargo.toml\` first`);
  }
  return JSON.parse(readFileSync(ACL_PATH, "utf8")) as Record<string, AclPlugin>;
}
const ACL = loadAcl();

type Grant = { allow: Set<string>; deny: Set<string> };

/** `core:window:allow-show` → commands `window|show`. Sets and defaults expand
 * through the manifest; members without a plugin prefix belong to the set's
 * own plugin. */
function grantOf(identifier: string, within?: string): Grant {
  const qualified = within && !identifier.includes(":") ? `${within}:${identifier}` : identifier;
  const split = qualified.lastIndexOf(":");
  const pluginKey = qualified.slice(0, split);
  const name = qualified.slice(split + 1);
  const plugin = ACL[pluginKey];
  if (!plugin) throw new Error(`unknown permission ${identifier}`);
  const members = name === "default" ? plugin.default_permission?.permissions : plugin.permission_sets[name]?.permissions;
  const grant: Grant = { allow: new Set(), deny: new Set() };
  if (members) {
    for (const member of members) {
      const inner = grantOf(member, pluginKey);
      for (const command of inner.allow) grant.allow.add(command);
      for (const command of inner.deny) grant.deny.add(command);
    }
    return grant;
  }
  const permission = plugin.permissions[name];
  if (!permission) throw new Error(`unknown permission ${identifier}`);
  const commandPlugin = pluginKey.replace(/^core:/, "");
  for (const command of permission.commands.allow) grant.allow.add(`${commandPlugin}|${command}`);
  for (const command of permission.commands.deny) grant.deny.add(`${commandPlugin}|${command}`);
  return grant;
}

function allowed(grants: Grant[]): Set<string> {
  const deny = new Set(grants.flatMap((grant) => [...grant.deny]));
  return new Set(grants.flatMap((grant) => [...grant.allow]).filter((command) => !deny.has(command)));
}

/** The manifest permission that allows exactly this command, for messages. */
function permissionFor(command: string): string {
  const [plugin, name] = command.split("|");
  const key = ACL[plugin!] ? plugin! : `core:${plugin}`;
  const found = Object.entries(ACL[key]?.permissions ?? {}).find(([, p]) => p.commands.allow.includes(name!));
  return found ? `${key}:${found[0]}` : `(no permission allows ${command})`;
}

type Capability = { identifier: string; windows: string[]; permissions: Array<string | { identifier: string }> };

const CAPABILITY_DIR = "src-tauri/capabilities";
const CAPABILITIES = readdirSync(CAPABILITY_DIR)
  .filter((name) => name.endsWith(".json"))
  .map((name) => JSON.parse(readFileSync(join(CAPABILITY_DIR, name), "utf8")) as Capability);

const entryId = (entry: string | { identifier: string }) => (typeof entry === "string" ? entry : entry.identifier);

// ---- Which windows exist ----------------------------------------------------

/** A label, or a template label's fixed prefix. A template is sampled with a
 * character no real label can hold (labels are `a-zA-Z0-9-/:_`), so only a
 * pattern that covers every suffix matches it. */
type CreatedWindow = { label: string; sample: string; source: string };

function labelOf(argument: string, text: string, at: number, source: string): CreatedWindow {
  const arg = argument.trim();
  const literal = arg.match(/^["']([^"']+)["']$/) ?? arg.match(/^`([^`$]+)`$/);
  if (literal) return { label: literal[1]!, sample: literal[1]!, source };
  const template = arg.match(/^`([^`$]*)\$\{/);
  if (template) return { label: `${template[1]}*`, sample: `${template[1]}\u0001`, source };
  const identifier = arg.split("::").pop()!;
  const definitions = [...text.slice(0, at).matchAll(new RegExp(`const\\s+${escapeRegExp(identifier)}\\s*(?::\\s*&str\\s*)?=\\s*([^;\\n]+)`, "g"))];
  const definition = definitions.at(-1) ?? [...text.matchAll(new RegExp(`const\\s+${escapeRegExp(identifier)}\\s*(?::\\s*&str\\s*)?=\\s*([^;\\n]+)`, "g"))][0];
  if (!definition) throw new Error(`${source}: cannot resolve window label ${arg}`);
  return labelOf(definition[1]!.replace(/;$/, ""), text, definition.index!, source);
}

function createdWindows(): CreatedWindow[] {
  const found: CreatedWindow[] = [];
  const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8")) as { app: { windows: Array<{ label?: string }> } };
  // Tauri labels a configured window without one "main".
  for (const window of config.app.windows) {
    const label = window.label ?? "main";
    found.push({ label, sample: label, source: "src-tauri/tauri.conf.json" });
  }
  for (const { path, text } of SOURCE_FILES) {
    const code = withoutComments(text);
    for (const match of code.matchAll(/\bnew\s+WebviewWindow\(\s*([^,)]+)/g)) {
      found.push(labelOf(match[1]!, code, match.index, path));
    }
  }
  for (const name of readdirSync("src-tauri/src", { recursive: true, encoding: "utf8" }).filter((n) => n.endsWith(".rs"))) {
    const path = join("src-tauri/src", name);
    const code = withoutComments(readFileSync(path, "utf8"));
    for (const match of code.matchAll(/WebviewWindowBuilder::new\(\s*[^,]+,\s*([^,]+),/g)) {
      found.push(labelOf(match[1]!, code, match.index, path));
    }
  }
  return found;
}

const WINDOWS = createdWindows();

function globMatches(pattern: string, label: string): boolean {
  return new RegExp(`^${pattern.split("*").map(escapeRegExp).join(".*")}$`, "s").test(label);
}

// ---- The checks, as pure functions so a planted violation can prove each ---

function ungrantedCalls(capabilities: Capability[], windows: CreatedWindow[], calls: Call[]): string[] {
  const problems: string[] = [];
  for (const window of windows) {
    const covering = capabilities.filter((capability) => capability.windows.some((pattern) => globMatches(pattern, window.sample)));
    const granted = allowed(covering.flatMap((capability) => capability.permissions.map((entry) => grantOf(entryId(entry)))));
    for (const call of calls) {
      for (const command of call.commands) {
        if (!granted.has(command)) problems.push(`${window.label}: ${call.file} ${call.call} needs ${permissionFor(command)}`);
      }
    }
  }
  return [...new Set(problems)];
}

/** An entry is needed only when it allows a called command that no other entry
 * of its capability allows: unused grants and duplicates both fail. */
function unneededEntries(capabilities: Capability[], used: Set<string>): string[] {
  const problems: string[] = [];
  for (const capability of capabilities) {
    const grants = capability.permissions.map((entry) => grantOf(entryId(entry)));
    capability.permissions.forEach((entry, index) => {
      const others = allowed(grants.filter((_, other) => other !== index));
      const unique = [...grants[index]!.allow].filter((command) => used.has(command) && !others.has(command));
      if (unique.length === 0) problems.push(`${capability.identifier}: ${entryId(entry)} allows nothing the frontend calls that other entries do not`);
    });
  }
  return problems;
}

function unmatchedWindows(capabilities: Capability[], windows: CreatedWindow[]): string[] {
  const problems: string[] = [];
  for (const window of windows) {
    if (!capabilities.some((capability) => capability.windows.some((pattern) => globMatches(pattern, window.sample)))) {
      problems.push(`${window.source} creates ${window.label}, which no capability covers`);
    }
  }
  for (const capability of capabilities) {
    for (const pattern of capability.windows) {
      if (!windows.some((window) => globMatches(pattern, window.sample))) {
        problems.push(`${capability.identifier} names window ${pattern}, which nothing creates`);
      }
    }
  }
  return problems;
}

const withPermissions = (capability: Capability, permissions: Capability["permissions"]): Capability => ({ ...capability, permissions });

describe("Tauri calls and granted capabilities", () => {
  it("probes the real Tauri packages, not the suite's doubles", () => {
    for (const module of Object.values(MODULES)) {
      for (const value of Object.values(module)) {
        if (typeof value === "function") expect(vi.isMockFunction(value)).toBe(false);
      }
    }
    expect([...SURFACES.values()].some((surface) => surface.methods.size > 0)).toBe(true);
  });

  it("finds calls in every file that imports a Tauri function or class", () => {
    // A file whose calls the scan cannot see would make every check below
    // pass by checking nothing.
    expect(FRONTEND.unseen).toEqual([]);
    expect(USED.size).toBeGreaterThan(0);
  });

  it("grants every command the frontend sends to every window", () => {
    expect(ungrantedCalls(CAPABILITIES, WINDOWS, FRONTEND.calls)).toEqual([]);
  });

  it("grants nothing the frontend does not call", () => {
    expect(unneededEntries(CAPABILITIES, USED)).toEqual([]);
  });

  it("covers exactly the windows the app creates", () => {
    expect(WINDOWS.length).toBeGreaterThan(1);
    expect(unmatchedWindows(CAPABILITIES, WINDOWS)).toEqual([]);
  });

  it("fails on a planted missing grant, unused grant, duplicate grant and window gap", () => {
    const [capability] = CAPABILITIES;
    const needed = capability!.permissions[0]!;
    expect(ungrantedCalls([withPermissions(capability!, capability!.permissions.slice(1))], WINDOWS, FRONTEND.calls)).not.toEqual([]);

    const granted = allowed(capability!.permissions.map((entry) => grantOf(entryId(entry))));
    const unusedPermission = Object.keys(ACL["core:window"]!.permissions)
      .map((name) => `core:window:${name}`)
      .find((id) => {
        const allows = [...grantOf(id).allow];
        return allows.length > 0 && allows.every((command) => !USED.has(command) && !granted.has(command));
      });
    expect(unneededEntries([withPermissions(capability!, [...capability!.permissions, unusedPermission!])], USED)).toEqual([
      `${capability!.identifier}: ${unusedPermission} allows nothing the frontend calls that other entries do not`,
    ]);
    expect(unneededEntries([withPermissions(capability!, [...capability!.permissions, needed])], USED)).toHaveLength(2);

    expect(unmatchedWindows([{ ...capability!, windows: capability!.windows.slice(1) }], WINDOWS)).toHaveLength(1);
    expect(unmatchedWindows([{ ...capability!, windows: [...capability!.windows, "planted-*"] }], WINDOWS)).toEqual([
      `${capability!.identifier} names window planted-*, which nothing creates`,
    ]);
  });

  it("keeps native fullscreen setters behind the platform-aware app command", () => {
    // Fullscreen is OneCopy's own app command; Tauri's window fullscreen is
    // macOS Spaces fullscreen, so no webview may set it directly.
    const window = SURFACES.get("@tauri-apps/api/window")!;
    const setters = [...window.methods].filter(([method]) => /^set\w*Fullscreen/.test(method)).flatMap(([, commands]) => commands);
    expect(setters.length).toBeGreaterThan(0);
    const granted = allowed(CAPABILITIES.flatMap((capability) => capability.permissions.map((entry) => grantOf(entryId(entry)))));
    expect(setters.filter((command) => granted.has(command))).toEqual([]);
  });
});

describe("failed window calls are reported, never swallowed", () => {
  it("uses no bare catch around a Tauri call", () => {
    // `.catch(() => {})` is what turned a missing permission into an
    // invisible no-op. These calls stay best-effort — a window the user just
    // closed must not throw — but the reason reaches the log.
    for (const source of SOURCES.filter((text) => /from ["']@tauri-apps\/api\/(?:window|webviewWindow)["']/.test(text))) {
      expect(source).not.toContain("catch(() => {})");
    }
  });
});
