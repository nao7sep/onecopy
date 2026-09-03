import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";

const parameters = JSON.parse(process.env.ONECOPY_AI_BENCHMARK_CASE);
const resultPath = process.env.ONECOPY_AI_CASE_RESULT;
const classId = {
  face: "faces",
  "audio-transcription": "audio-transcripts",
  "video-transcription": "video-transcripts",
}[parameters.id];

async function invoke(command, args = {}) {
  return browser.tauri.execute(
    ({ core }, name, input) => core.invoke(name, input),
    command,
    args,
  );
}

async function allItems() {
  const counts = await invoke("get_section_counts");
  const sections = [["image", counts.images], ["video", counts.videos], ["other", counts.others]];
  const items = [];
  for (const [kind, months] of sections) {
    for (const { month, count } of months) {
      const window = await invoke("get_section_window", {
        kind,
        month,
        sort: { order: "name", desc: false },
        start: 0,
        limit: count,
      });
      items.push(...window.items);
    }
  }
  return items;
}

function phraseLoop(text) {
  const segments = text
    .split(/\r?\n/)
    .map((line) => line.replace(/^\[[^\]]+\]\s*/, "").trim().toLowerCase().split(/\s+/).filter(Boolean))
    .filter((tokens) => tokens.length > 0);
  if (segments.some((tokens, index) => index > 0 && tokens.join(" ") === segments[index - 1].join(" "))) {
    return true;
  }
  const tokens = segments.flat();
  for (let width = 3; width <= Math.min(16, Math.floor(tokens.length / 3)); width += 1) {
    for (let start = 0; start <= tokens.length - width * 3; start += 1) {
      const phrase = tokens.slice(start, start + width).join(" ");
      if (
        phrase === tokens.slice(start + width, start + width * 2).join(" ") &&
        phrase === tokens.slice(start + width * 2, start + width * 3).join(" ")
      ) return true;
    }
  }
  return false;
}

describe("OneCopy parameterized AI benchmark case", () => {
  it(`runs ${parameters.id} through the complete application`, async () => {
    const started = performance.now();
    const menu = await $('button[aria-label="Open menu"]');
    await menu.waitForDisplayed({ timeout: 5 * 60_000 });
    const appReadyMs = performance.now() - started;

    const discoveryStarted = performance.now();
    await browser.waitUntil(
      async () => {
        const snapshot = await invoke("index_work_snapshot");
        return snapshot.sourceCheck.eventSequence > 0 &&
          !snapshot.sourceCheck.running &&
          !snapshot.fileInformation.running &&
          !snapshot.fileInformation.queued;
      },
      { interval: 1_000, timeoutMsg: "source discovery and file information did not settle" },
    );
    const sourceDiscoveryMs = performance.now() - discoveryStarted;

    await invoke("admit_background_completion");
    const queueStarted = performance.now();
    let lastReport = 0;
    await browser.waitUntil(
      async () => {
        const snapshot = await invoke("background_work_snapshot");
        const item = snapshot.classes.find(({ id }) => id === classId);
        assert(item, `background class ${classId} exists`);
        if (performance.now() - lastReport > 60_000) {
          console.log(`${classId}: ${item.state}; queued=${item.queued}; done=${item.done}; failed=${item.failed}`);
          lastReport = performance.now();
        }
        assert.equal(item.failed, 0, `${classId} has no failures`);
        return item.state === "up-to-date" && item.queued === 0;
      },
      { interval: 2_000, timeoutMsg: `${classId} did not settle` },
    );
    const queueWaitMs = performance.now() - queueStarted;

    const readbackStarted = performance.now();
    const items = await allItems();
    assert.equal(items.length, parameters.fixtures.length, "the isolated source has the requested fixtures only");
    const byName = new Map(items.map((item) => [item.fileName, item]));
    let correctness;
    let normalizedOutputSha256;
    if (parameters.id === "face") {
      const scores = parameters.fixtures.map(({ basename }) => {
        const item = byName.get(basename);
        assert(item, `${basename} is indexed`);
        assert.equal(item.derivedWork.faces.state, "ready");
        assert(Number.isFinite(item.faceScore), `${basename} has a finite face score`);
        assert(item.faceScore >= parameters.oracle.minimumScore);
        assert(item.faceScore <= parameters.oracle.maximumScore);
        return item.faceScore;
      });
      assert(scores.length >= parameters.oracle.minimumReady);
      correctness = { ready: scores.length, total: scores.length };
    } else {
      const fixture = parameters.fixtures[0];
      const item = byName.get(fixture.basename);
      assert(item, `${fixture.basename} is indexed`);
      assert.equal(item.derivedWork.transcripts.state, "ready");
      const transcript = await invoke("transcript_get", { hash: item.hash });
      assert.equal(transcript.status, "ready");
      const normalized = transcript.text.toLowerCase();
      const matchedTerms = parameters.oracle.semanticTerms.filter((term) =>
        normalized.includes(term.toLowerCase()),
      ).length;
      assert(matchedTerms >= parameters.oracle.minimumTermMatches, `matched ${matchedTerms} semantic terms`);
      assert(!parameters.oracle.rejectPhraseLoop || !phraseLoop(transcript.text), "transcript has no phrase loop");
      normalizedOutputSha256 = createHash("sha256").update(normalized).digest("hex");
      correctness = {
        matchedTerms,
        segmentCount: transcript.text.split(/\r?\n/).filter(Boolean).length,
        phraseLoop: false,
      };
    }
    const ipcReadbackMs = performance.now() - readbackStarted;
    writeFileSync(resultPath, `${JSON.stringify({
      requestedAcceleration: parameters.acceleration,
      correctness,
      phases: { appReadyMs, sourceDiscoveryMs, queueWaitMs, ipcReadbackMs },
      ...(normalizedOutputSha256 ? { normalizedOutputSha256 } : {}),
    }, null, 2)}\n`);
  });
});
