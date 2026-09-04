import assert from "node:assert/strict";
import test from "node:test";
import { jsonLineEvents } from "./progress.mjs";

test("progress relay accepts split JSON lines and ignores incidental private output", () => {
  const events = [];
  const relay = jsonLineEvents((event) => events.push(event));
  relay.push("native diagnostic: /Users/private/model.bin\n{\"event\":\"scenario-pro");
  relay.push("gress\",\"percent\":42}\nnot json\n{\"event\":\"done\"}");
  relay.finish();
  assert.deepEqual(events, [
    { event: "scenario-progress", percent: 42 },
    { event: "done" },
  ]);
});
