import { copyFileSync, existsSync, linkSync, lstatSync, mkdirSync, readdirSync } from "node:fs";
import { join } from "node:path";

/** Clones verified, immutable prepared artifacts cheaply on the same volume. */
export function cloneArtifactTree(source, target) {
  mkdirSync(target, { recursive: true });
  for (const entry of readdirSync(source, { withFileTypes: true })) {
    const from = join(source, entry.name);
    const to = join(target, entry.name);
    const stat = lstatSync(from);
    if (stat.isSymbolicLink()) throw new Error("prepared artifacts may not contain symlinks");
    if (stat.isDirectory()) {
      cloneArtifactTree(from, to);
    } else if (stat.isFile()) {
      if (existsSync(to)) continue;
      try {
        linkSync(from, to);
      } catch {
        copyFileSync(from, to);
      }
    }
  }
}
