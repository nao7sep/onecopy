// Source-level IPC checks use a TypeScript-aware parser so shorthand, nested
// values and punctuation inside strings cannot hide command argument drift.
import { parse } from "@babel/parser";

type SyntaxNode = { type: string; [key: string]: unknown };
function node(value: unknown): value is SyntaxNode {
  return typeof value === "object" && value !== null && "type" in value && typeof value.type === "string";
}
function walk(value: unknown, visit: (value: SyntaxNode) => void): void {
  if (Array.isArray(value)) {
    for (const child of value) walk(child, visit);
  } else if (node(value)) {
    visit(value);
    for (const child of Object.values(value)) walk(child, visit);
  }
}

export interface InvokeCall {
  command: string | null;
  keys: string[];
  unresolved: string[];
}
export function invokeCallsIn(source: string): InvokeCall[] {
  const ast = parse(source, { sourceType: "module", plugins: ["typescript", "jsx"] });
  const names = new Set(["invoke"]);
  walk(ast, (value) => {
    if (value.type !== "ImportDeclaration" || !node(value.source) || value.source.value !== "@tauri-apps/api/core") return;
    for (const specifier of value.specifiers as unknown[]) {
      if (node(specifier) && node(specifier.imported) && specifier.imported.name === "invoke" && node(specifier.local)) {
        names.add(String(specifier.local.name));
      }
    }
  });
  const calls: InvokeCall[] = [];
  walk(ast, (value) => {
    if (value.type !== "CallExpression" || !node(value.callee) || value.callee.type !== "Identifier" || !names.has(String(value.callee.name))) return;
    const args = value.arguments as unknown[];
    const command = node(args[0]) && args[0].type === "StringLiteral" ? String(args[0].value) : null;
    const call: InvokeCall = { command, keys: [], unresolved: [] };
    const readObject = (argument: unknown) => {
      if (!node(argument) || argument.type !== "ObjectExpression") {
        call.unresolved.push("nonliteral arguments");
        return;
      }
      for (const property of argument.properties as unknown[]) {
        if (!node(property)) continue;
        if (property.type === "SpreadElement") {
          readObject(property.argument);
        } else if (property.type === "ObjectProperty" && node(property.key)) {
          if (property.key.type === "StringLiteral") call.keys.push(String(property.key.value));
          else if (!property.computed && property.key.type === "Identifier") call.keys.push(String(property.key.name));
          else call.unresolved.push("computed argument key");
        } else call.unresolved.push("non-value argument property");
      }
    };
    if (args.length > 1) readObject(args[1]);
    calls.push(call);
  });
  return calls;
}

export interface CommandParameter { key: string; required: boolean }
export function commandParameters(source: string): Map<string, CommandParameter[]> {
  const commands = new Map<string, CommandParameter[]>();
  for (const match of source.matchAll(/#\[tauri::command\]\s*(?:async\s+)?fn\s+(\w+)\s*\(/g)) {
    const start = match.index + match[0].length;
    let depth = 1;
    let end = start;
    for (; end < source.length && depth > 0; end++) {
      if (source[end] === "(") depth++;
      if (source[end] === ")") depth--;
    }
    if (depth !== 0) throw new Error(`Unclosed command signature: ${match[1]}`);
    const raw = source.slice(start, end - 1);
    const parameters: string[] = [];
    let from = 0;
    let nested = 0;
    for (let i = 0; i <= raw.length; i++) {
      if (i === raw.length || raw[i] === "," && nested === 0) {
        parameters.push(raw.slice(from, i).trim()); from = i + 1;
      } else if ("<([{".includes(raw[i]!)) nested++;
      else if (">)]}".includes(raw[i]!)) nested--;
    }
    commands.set(match[1]!, parameters.filter(Boolean).flatMap((parameter) => {
      const parsed = /^(\w+)\s*:\s*([\s\S]+)$/.exec(parameter);
      if (!parsed) throw new Error(`Unsupported command parameter: ${parameter}`);
      const [, name, type] = parsed;
      // Injection is a property of the Rust type, never its variable name.
      if (/^(?:tauri::)?(?:AppHandle|Window|WebviewWindow|State)\b/.test(type!)) return [];
      return [{ key: name!.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase()), required: !/^Option\s*</.test(type!) }];
    }));
  }
  return commands;
}

export function argumentProblems(call: InvokeCall, parameters: CommandParameter[]): string[] {
  return [
    ...call.unresolved,
    ...call.keys.filter((key) => !parameters.some((parameter) => parameter.key === key)).map((key) => `unknown ${key}`),
    ...parameters.filter((parameter) => parameter.required && !call.keys.includes(parameter.key)).map((parameter) => `missing ${parameter.key}`),
  ];
}
