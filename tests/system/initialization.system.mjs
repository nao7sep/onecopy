import assert from "node:assert/strict";

// This suite crosses the real packaged-app boundary; model engines are covered
// separately so a failure here identifies initialization or IPC wiring.

describe("running OneCopy", () => {
  it("initializes the application and exposes the platform AI dependencies", async () => {
    const menu = await $('button[aria-label="Open menu"]');
    await menu.waitForDisplayed({ timeout: 5 * 60_000 });
    await menu.click();

    const managedTools = await $('//*[normalize-space(.)="Managed tools…"]');
    await managedTools.waitForDisplayed();
    await managedTools.click();

    await browser.waitUntil(
      async () => (await $("body").getText()).includes("Models selected by OneCopy"),
      { timeoutMsg: "Managed tools did not finish loading" },
    );
    const body = await $("body").getText();

    assert.match(body, /ffmpeg\s*Not installed/);
    assert.match(body, /Transcription model \(Whisper large-v3-turbo\)\s*Not installed/);
    assert.match(body, /Face detector\s*Not installed/);
    assert.match(body, /Expression model\s*Not installed/);
    if (process.platform === "win32") {
      assert.match(body, /Face-scoring runtime \(ONNX Runtime 1\.28\)\s*Not installed/);
    } else {
      assert.doesNotMatch(body, /Face-scoring runtime/);
    }
    assert.doesNotMatch(body, /Setup\s+Step 1 of 3/);

    const work = await browser.tauri.execute(({ core }) =>
      core.invoke("background_work_snapshot"),
    );
    assert.deepEqual(
      work.classes.map(({ id }) => id),
      ["previews", "snapshots", "similarity", "faces", "video-transcripts", "audio-transcripts"],
      "the renderer reaches the real initialized Rust coordinator",
    );
  });
});
