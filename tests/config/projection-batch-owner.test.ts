// One owner arms the logical-projection batch guard (R7-02).
//
// `logical_projection_batch` suppresses the per-row projection triggers, and
// whoever arms it must drop and republish every touched hash in the right
// order. `index_store::publish_paths_batch_in` is that single owner; a second
// hand-written copy of the protocol is the drift this source-text check stops.

import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const sourceDir = fileURLToPath(new URL("../../src-tauri/src/", import.meta.url));

function guardWrites(): Array<{ file: string; count: number }> {
  const writes = /(?:INSERT\s+INTO|DELETE\s+FROM)\s+logical_projection_batch\b/g;
  return readdirSync(sourceDir, { recursive: true, encoding: "utf8" })
    .filter((file) => file.endsWith(".rs"))
    .map((file) => ({
      file,
      count: readFileSync(`${sourceDir}${file}`, "utf8").match(writes)?.length ?? 0,
    }))
    .filter((entry) => entry.count > 0);
}

describe("projection batch guard", () => {
  it("is armed and cleared only by the batch publisher", () => {
    expect(guardWrites()).toEqual([{ file: "index_store.rs", count: 2 }]);
  });
});
