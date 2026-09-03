import { mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";

const home = process.env.ONECOPY_E2E_HOME ??
  join(tmpdir(), "onecopy-wdio-acceptance");
const source = join(home, "source");

function assertOneCopyIsClosed() {
  const probe = process.platform === "win32"
    ? spawnSync("tasklist.exe", ["/FI", "IMAGENAME eq onecopy.exe", "/FO", "CSV", "/NH"], {
        encoding: "utf8",
      })
    : spawnSync("pgrep", ["-ix", "onecopy"], { encoding: "utf8" });
  const running = process.platform === "win32"
    ? probe.stdout.toLowerCase().includes("onecopy.exe")
    : probe.status === 0 && probe.stdout.trim().length > 0;
  if (running) {
    throw new Error("Close every running OneCopy window before preparing its isolated E2E home.");
  }
}

function assertHome(path) {
  const resolved = resolve(path);
  const expectedParent = resolve(tmpdir());
  const expectedName = "onecopy-wdio-acceptance";
  if (
    basename(resolved) !== expectedName ||
    !resolved.startsWith(`${expectedParent}${process.platform === "win32" ? "\\" : "/"}`)
  ) {
    throw new Error(`Refusing to manage unexpected E2E home: ${resolved}`);
  }
  return resolved;
}

const root = assertHome(home);
assertOneCopyIsClosed();
mkdirSync(root, { recursive: true });
for (const name of readdirSync(root)) {
  rmSync(join(root, name), { recursive: true, force: true });
}
mkdirSync(source, { recursive: true });

writeFileSync(
  join(root, "config.json"),
  `${JSON.stringify(
    {
      sourceDirs: [source],
      defaultTimezone: "UTC",
      checkSourceFoldersAtLaunch: false,
      checkUpdatesAtLaunch: false,
      keepAwakeDuringIndexing: false,
      videoSnapshotsEnabled: false,
      similarPhotoAnalysisEnabled: false,
      scoreFaces: false,
      videoTranscriptionEnabled: false,
      audioTranscriptionEnabled: false,
    },
    null,
    2,
  )}\n`,
);
console.log(`Prepared initialization E2E home: ${root}`);
