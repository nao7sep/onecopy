import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

function git(root, args, encoding = "utf8") {
  return execFileSync("git", ["-C", root, ...args], {
    encoding,
    windowsHide: true,
    timeout: 30_000,
    maxBuffer: 64 * 1024 * 1024,
  });
}

export function sourceState(root) {
  const commit = git(root, ["rev-parse", "HEAD"]).trim();
  const status = git(root, ["status", "--porcelain=v1", "--untracked-files=normal"]);
  const diff = git(root, ["diff", "--binary", "HEAD", "--"], null);
  const untracked = git(root, ["ls-files", "--others", "--exclude-standard", "-z"], null)
    .toString("utf8")
    .split("\0")
    .filter(Boolean)
    .sort();
  const untrackedDigest = createHash("sha256");
  for (const file of untracked) {
    untrackedDigest.update(file);
    untrackedDigest.update("\0");
    untrackedDigest.update(readFileSync(resolve(root, file)));
    untrackedDigest.update("\0");
  }
  return {
    commit,
    dirty: status.length > 0,
    trackedDiffSha256: createHash("sha256").update(diff).digest("hex"),
    untrackedCount: untracked.length,
    untrackedContentSha256: untrackedDigest.digest("hex"),
  };
}
