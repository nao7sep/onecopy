import { createHash } from "node:crypto";
import { constants, copyFileSync, lstatSync, mkdirSync, openSync, readSync, closeSync, readdirSync } from "node:fs";
import { basename, isAbsolute, join, relative, resolve, sep } from "node:path";

export function sha256File(path) {
  const hash = createHash("sha256");
  const fd = openSync(path, "r");
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  try {
    for (;;) {
      const count = readSync(fd, buffer, 0, buffer.length, null);
      if (count === 0) break;
      hash.update(buffer.subarray(0, count));
    }
  } finally {
    closeSync(fd);
  }
  return hash.digest("hex");
}

export function indexFixtureRoot(root) {
  const resolvedRoot = resolve(root);
  const rootStat = lstatSync(resolvedRoot);
  if (!rootStat.isDirectory() || rootStat.isSymbolicLink()) {
    throw new Error("fixture root must be a real directory");
  }
  const index = new Map();
  const stack = [resolvedRoot];
  while (stack.length > 0) {
    const directory = stack.pop();
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isSymbolicLink()) continue;
      if (entry.isDirectory()) {
        stack.push(path);
      } else if (entry.isFile()) {
        const exact = index.get(entry.name) ?? [];
        exact.push(path);
        index.set(entry.name, exact);
        const foldedKey = `folded:${entry.name.toLocaleLowerCase("en-US")}`;
        const folded = index.get(foldedKey) ?? [];
        folded.push(entry.name);
        index.set(foldedKey, folded);
      }
    }
  }
  return { root: resolvedRoot, index };
}

export function resolveFixtures(indexed, references) {
  const resolved = [];
  const errors = [];
  for (const reference of references) {
    const folded = indexed.index.get(`folded:${reference.basename.toLocaleLowerCase("en-US")}`) ?? [];
    if (folded.some((name) => name !== reference.basename)) {
      errors.push(`${reference.basename}: case-variant filename exists`);
      continue;
    }
    const candidates = indexed.index.get(reference.basename) ?? [];
    const matches = [];
    for (const path of candidates) {
      let stat;
      try {
        stat = lstatSync(path);
      } catch {
        errors.push(`${reference.basename}: file became unreadable`);
        continue;
      }
      if (!stat.isFile() || stat.isSymbolicLink() || stat.size !== reference.bytes) continue;
      if (sha256File(path) === reference.sha256) matches.push(path);
    }
    if (matches.length !== 1) {
      errors.push(`${reference.basename}: expected exactly one basename-and-hash match, found ${matches.length}`);
    } else {
      resolved.push({ reference, path: matches[0] });
    }
  }
  if (errors.length) throw new Error(`fixture preflight failed: ${errors.join("; ")}`);
  return resolved;
}

function assertOwnedChild(root, child) {
  const rel = relative(resolve(root), resolve(child));
  if (rel === "" || rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel)) {
    throw new Error("fixture destination escaped the disposable source root");
  }
}

export function materializeFixtures(root, resolvedFixtures) {
  const source = resolve(root);
  mkdirSync(source, { recursive: true });
  const outputs = [];
  for (const fixture of resolvedFixtures) {
    const target = join(source, fixture.reference.basename);
    assertOwnedChild(source, target);
    if (basename(target) !== fixture.reference.basename) {
      throw new Error("fixture basename changed during materialization");
    }
    const before = lstatSync(fixture.path);
    if (!before.isFile() || before.isSymbolicLink() || sha256File(fixture.path) !== fixture.reference.sha256) {
      throw new Error(`${fixture.reference.basename}: source changed after preflight`);
    }
    copyFileSync(fixture.path, target, constants.COPYFILE_EXCL);
    if (sha256File(target) !== fixture.reference.sha256) {
      throw new Error(`${fixture.reference.basename}: copied digest mismatch`);
    }
    outputs.push(target);
  }
  return outputs;
}
