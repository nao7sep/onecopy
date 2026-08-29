import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const INVALID_STEMS = new Set(["general", "misc", "overview", "reference", "system"]);
const ROUTING_HEADER = "| File | Solely owns | Explicitly excludes |";
const ROUTING_SEPARATOR = "|---|---|---|";

function markdownFiles(root) {
  const files = [];
  if (!existsSync(root)) return files;
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const absolute = path.join(root, entry.name);
    if (entry.isDirectory()) {
      files.push(...markdownFiles(absolute));
    } else if (entry.isFile() && entry.name.toLowerCase().endsWith(".md")) {
      files.push(absolute);
    }
  }
  return files;
}

function relativeSpecPath(specsRoot, file) {
  return path.relative(specsRoot, file).split(path.sep).join("/");
}

function localReferences(markdown) {
  const references = [];
  for (const match of markdown.matchAll(/\]\(([^)]+)\)/g)) references.push(match[1]);
  for (const line of markdown.split(/\r?\n/)) {
    const match = /^\s*\[[^\]]+\]:\s*(\S+)/.exec(line);
    if (match) references.push(match[1]);
  }
  return references
    .map((reference) => reference.replace(/^<|>$/g, "").split("#", 1)[0])
    .filter(
      (reference) =>
        reference !== "" &&
        !reference.startsWith("#") &&
        !/^[a-z][a-z0-9+.-]*:/i.test(reference),
    );
}

export function specStructureErrors(repositoryRoot) {
  const specsRoot = path.join(repositoryRoot, "specs");
  const indexPath = path.join(specsRoot, "index.md");
  const errors = [];
  const files = markdownFiles(specsRoot);

  if (!existsSync(indexPath)) {
    return ["specs/index.md is missing"];
  }

  for (const file of files) {
    const relative = relativeSpecPath(specsRoot, file);
    const markdown = readFileSync(file, "utf8");
    if (markdown.trim() === "") errors.push(`specs/${relative} is empty`);
    const stem = path.basename(file, path.extname(file)).toLowerCase();
    if (relative !== "index.md" && INVALID_STEMS.has(stem)) {
      errors.push(`specs/${relative} uses an invalid catch-all filename`);
    }
    for (const reference of localReferences(markdown)) {
      const target = path.resolve(path.dirname(file), reference);
      if (!existsSync(target) || !statSync(target).isFile()) {
        errors.push(`specs/${relative} has a broken reference: ${reference}`);
      }
    }
  }

  const index = readFileSync(indexPath, "utf8");
  if ((index.match(/^# Spec Index\s*$/gm) ?? []).length !== 1) {
    errors.push("specs/index.md must contain exactly one # Spec Index heading");
  }
  const lines = index.split(/\r?\n/).map((line) => line.trim());
  const routingTables = lines.filter(
    (line, index) => line === ROUTING_HEADER && lines[index + 1] === ROUTING_SEPARATOR,
  ).length;
  if (routingTables !== 1) {
    errors.push("specs/index.md must contain exactly one routing table");
  }

  const routes = [...index.matchAll(/^\|\s*`([^`]+\.md)`\s*\|/gm)].map((match) =>
    path.posix.normalize(match[1].replaceAll("\\", "/")),
  );
  if (routes.length === 0) errors.push("specs/index.md routes no contract files");
  if (routes.includes("index.md")) errors.push("specs/index.md must not route itself");

  const counts = new Map();
  for (const route of routes) counts.set(route, (counts.get(route) ?? 0) + 1);
  for (const [route, count] of counts) {
    if (count > 1) errors.push(`specs/${route} is routed ${count} times`);
  }

  const contracts = files
    .map((file) => relativeSpecPath(specsRoot, file))
    .filter((file) => file !== "index.md");
  for (const contract of contracts) {
    if (!counts.has(contract)) errors.push(`specs/${contract} is not routed`);
  }
  for (const route of counts.keys()) {
    if (!contracts.includes(route)) errors.push(`specs/index.md routes missing specs/${route}`);
  }

  return [...new Set(errors)];
}

if (path.resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  const root = path.resolve(process.argv[2] ?? process.cwd());
  const errors = specStructureErrors(root);
  if (errors.length > 0) {
    process.stderr.write(`${errors.join("\n")}\n`);
    process.exitCode = 1;
  } else {
    process.stdout.write("Spec structure is valid.\n");
  }
}
