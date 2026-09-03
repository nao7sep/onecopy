import assert from "node:assert/strict";

// This suite crosses the real packaged-app boundary after the direct model
// fixtures pass, isolating admission, queue, persistence, and presentation.

const transcriptionAcceptance =
  (process.env.ONECOPY_AI_ACCEPTANCE_FEATURES ?? "all") === "all";
const EXPECTED = transcriptionAcceptance
  ? { faces: 1, "video-transcripts": 1, "audio-transcripts": 1 }
  : { faces: 1 };

async function invoke(command, args = {}) {
  return browser.tauri.execute(
    ({ core }, name, parameters) => core.invoke(name, parameters),
    command,
    args,
  );
}

async function allItems() {
  const counts = await invoke("get_section_counts");
  const sections = [
    ["image", counts.images],
    ["video", counts.videos],
    ["other", counts.others],
  ];
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

describe("OneCopy AI acceptance with production artifacts", () => {
  it("admits, runs, persists, and exposes every AI result", async () => {
    const menu = await $('button[aria-label="Open menu"]');
    await menu.waitForDisplayed({ timeout: 5 * 60_000 });

    await browser.waitUntil(
      async () => {
        const snapshot = await invoke("index_work_snapshot");
        return (
          snapshot.sourceCheck.eventSequence > 0 &&
          !snapshot.sourceCheck.running &&
          !snapshot.fileInformation.running &&
          !snapshot.fileInformation.queued
        );
      },
      {
        interval: 2_000,
        timeoutMsg: "source check and file-information work did not settle",
      },
    );

    await invoke("admit_background_completion");
    let lastReport = 0;
    await browser.waitUntil(
      async () => {
        const snapshot = await invoke("background_work_snapshot");
        const now = Date.now();
        if (now - lastReport >= 60_000) {
          console.log(
            "AI queue:",
            snapshot.classes
              .filter(({ id }) => id in EXPECTED)
              .map(({ id, state, queued, done, failed }) =>
                `${id}=${state} queued:${queued} done:${done} failed:${failed}`,
              )
              .join(", "),
          );
          lastReport = now;
        }
        return Object.keys(EXPECTED).every((id) => {
          const item = snapshot.classes.find((candidate) => candidate.id === id);
          assert(item, `background class ${id} exists`);
          assert.equal(item.failed, 0, `${id} has no failures`);
          // The embedded app starts before the WebDriver worker attaches, so
          // very fast work may be durably complete without a live-session
          // `done` counter. The item receipts asserted below are the authority.
          return item.state === "up-to-date" && item.queued === 0;
        });
      },
      { interval: 5_000, timeoutMsg: "AI background work did not settle" },
    );

    const items = await allItems();
    assert.equal(items.length, 3, "the isolated source contains exactly three logical items");
    const face = items.find(({ fileName }) => fileName === "acceptance-face.jpg");
    const audio = items.find(({ fileName }) => fileName === "acceptance-audio.flac");
    const video = items.find(({ fileName }) => fileName === "acceptance-video.mp4");
    assert(face && audio && video, "all three named fixtures are indexed");

    assert.equal(face.derivedWork.faces.state, "ready");
    assert(face.faceScore >= 0.5 && face.faceScore <= 1, `face score ${face.faceScore}`);

    if (transcriptionAcceptance) {
      for (const item of [audio, video]) {
        assert.equal(item.derivedWork.transcripts.state, "ready");
        const transcript = await invoke("transcript_get", { hash: item.hash });
        assert.equal(transcript.status, "ready");
        const text = transcript.text.toLowerCase();
        assert(text.includes("photo"), `${item.fileName} contains the canonical first sentence`);
        console.log(`${item.fileName}: ${transcript.text.replaceAll("\n", " ")}`);
      }
    }
    console.log(`acceptance-face.jpg: face score ${face.faceScore.toFixed(6)}`);
  });
});
