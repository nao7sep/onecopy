import { existsSync, readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const EXTENSIONS = new Set([
  ".axaml", ".cjs", ".command", ".cs", ".csproj", ".css", ".cts", ".html",
  ".htm", ".iss", ".js", ".json", ".jsonc", ".jsx", ".manifest", ".md",
  ".mjs", ".mts", ".plist", ".props", ".ps1", ".pubxml", ".py", ".rs",
  ".scss", ".sh", ".slnx", ".sql", ".svg", ".targets", ".toml", ".ts",
  ".tsx", ".txt", ".webmanifest", ".xaml", ".xml", ".yaml", ".yml",
]);
const FILENAMES = new Set([
  ".dockerignore", ".gitattributes", ".gitignore", ".hidden-char-scan-ignore",
  ".npmrc", ".nvmrc", ".vscodeignore",
]);
const SKIPPED_DIRECTORIES = new Set([
  ".git", ".gradle", ".idea", ".mypy_cache", ".next", ".nuxt", ".pytest_cache",
  ".turbo", ".venv", "Pods", "__pycache__", "bin", "build", "coverage", "dist",
  "node_modules", "obj", "out", "release", "target", "venv",
]);
const IGNORE_FILE = ".hidden-char-scan-ignore";

function ignoredPaths(root) {
  const ignoreFile = path.join(root, IGNORE_FILE);
  if (!existsSync(ignoreFile)) return [];
  return readFileSync(ignoreFile, "utf8")
    .split(/\r?\n/)
    .map((line) => line.replace(/#.*$/, "").trim())
    .filter(Boolean)
    .map((entry) => path.resolve(root, entry));
}

function sourceFiles(root, inheritedIgnores = new Set()) {
  const files = [];
  const ignores = new Set([...inheritedIgnores, ...ignoredPaths(root)]);
  const entries = readdirSync(root, { withFileTypes: true });
  entries.sort((a, b) => a.name.localeCompare(b.name));
  for (const entry of entries) {
    if (entry.isDirectory() && SKIPPED_DIRECTORIES.has(entry.name)) continue;
    const absolute = path.join(root, entry.name);
    if (ignores.has(absolute) || entry.isSymbolicLink()) continue;
    if (entry.isDirectory()) files.push(...sourceFiles(absolute, ignores));
    else if (
      entry.isFile() &&
      (EXTENSIONS.has(path.extname(entry.name).toLowerCase()) || FILENAMES.has(entry.name))
    ) {
      files.push(absolute);
    }
  }
  return files;
}

function forbidden(codePoint) {
  return (
    (codePoint <= 0x1f && ![0x09, 0x0a, 0x0d].includes(codePoint)) ||
    codePoint === 0x7f ||
    (codePoint >= 0x200b && codePoint <= 0x200f) ||
    codePoint === 0x2028 ||
    codePoint === 0x2029 ||
    (codePoint >= 0x202a && codePoint <= 0x202e) ||
    (codePoint >= 0x2066 && codePoint <= 0x2069) ||
    codePoint === 0xfeff
  );
}

export function hiddenCharacterErrors(repositoryRoot) {
  const errors = [];
  for (const file of sourceFiles(repositoryRoot)) {
    let text;
    try {
      text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(readFileSync(file));
    } catch {
      errors.push(`${path.relative(repositoryRoot, file)} is not valid UTF-8`);
      continue;
    }
    let line = 1;
    let column = 1;
    let offset = 0;
    for (const character of text) {
      const codePoint = character.codePointAt(0);
      const leadingBom = offset === 0 && codePoint === 0xfeff;
      if (!leadingBom && forbidden(codePoint)) {
        errors.push(
          `${path.relative(repositoryRoot, file)}:${line}:${column} contains U+${codePoint.toString(16).toUpperCase().padStart(4, "0")}`,
        );
      }
      if (character === "\n") {
        line += 1;
        column = 1;
      } else {
        column += 1;
      }
      offset += character.length;
    }
  }
  return errors;
}

if (path.resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  const root = path.resolve(process.argv[2] ?? process.cwd());
  const errors = hiddenCharacterErrors(root);
  if (errors.length > 0) {
    process.stderr.write(`${errors.join("\n")}\n`);
    process.exitCode = 1;
  } else {
    process.stdout.write("No hidden characters found.\n");
  }
}
