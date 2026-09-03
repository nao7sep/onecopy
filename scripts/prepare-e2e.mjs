import { copyFileSync, mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";

const aiAcceptance = process.env.ONECOPY_AI_ACCEPTANCE === "1";
const transcriptionAcceptance =
  (process.env.ONECOPY_AI_ACCEPTANCE_FEATURES ?? "all") === "all";
const home = process.env.ONECOPY_E2E_HOME ??
  (aiAcceptance
    ? resolve("src-tauri/target/ai-acceptance-home")
    : join(tmpdir(), "onecopy-wdio-acceptance"));
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
  const expectedParent = aiAcceptance
    ? resolve("src-tauri/target")
    : resolve(tmpdir());
  const expectedName = aiAcceptance
    ? "ai-acceptance-home"
    : "onecopy-wdio-acceptance";
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
  if (
    !aiAcceptance ||
    name === "source" ||
    name === "cache" ||
    name === "logs" ||
    name === "config.json" ||
    name === "state.json" ||
    name === "index.sqlite3" ||
    name.startsWith("index.sqlite3-")
  ) {
    rmSync(join(root, name), { recursive: true, force: true });
  }
}
mkdirSync(source, { recursive: true });

if (aiAcceptance) {
  const fixtures = resolve("../company/assets/test-fixtures");
  copyFileSync(
    join(fixtures, "photos/faces/face-01-reference.jpg"),
    join(source, "acceptance-face.jpg"),
  );
  const media = [
    ["audio/dialogue/dialogue-english-with-noise.flac", "acceptance-audio.flac"],
    ["video/dialogue/dialogue-english-with-noise.mp4", "acceptance-video.mp4"],
  ];
  if (transcriptionAcceptance) {
    const ffmpeg = join(root, "bin", process.platform === "win32" ? "ffmpeg.exe" : "ffmpeg");
    for (const [from, to] of media) {
      const result = spawnSync(
        ffmpeg,
        ["-hide_banner", "-loglevel", "error", "-y", "-i", join(fixtures, from), "-t", "4", "-map", "0", "-c", "copy", join(source, to)],
        { encoding: "utf8", timeout: 2 * 60_000 },
      );
      if (result.status !== 0) {
        throw new Error(`Could not create the four-second ${to} fixture: ${result.stderr}`);
      }
      console.log(`Prepared four-second AI fixture: ${to}`);
    }
  } else {
    for (const [from, to] of media) {
      copyFileSync(join(fixtures, from), join(source, to));
    }
  }
}

writeFileSync(
  join(root, "config.json"),
  `${JSON.stringify(
    {
      sourceDirs: [source],
      defaultTimezone: "UTC",
      checkSourceFoldersAtLaunch: aiAcceptance,
      checkUpdatesAtLaunch: false,
      keepAwakeDuringIndexing: false,
      videoSnapshotsEnabled: false,
      similarPhotoAnalysisEnabled: false,
      scoreFaces: aiAcceptance,
      videoTranscriptionEnabled: aiAcceptance && transcriptionAcceptance,
      audioTranscriptionEnabled: aiAcceptance && transcriptionAcceptance,
    },
    null,
    2,
  )}\n`,
);
console.log(`Prepared ${aiAcceptance ? "AI" : "initialization"} E2E home: ${root}`);
