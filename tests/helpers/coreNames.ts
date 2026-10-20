// The names the core sends and the frontend keys on: menu events, item keys,
// background-work ids, states and reasons, transcript statuses and events.
// The Rust suite checks this fixture against what the core sends; frontend
// tests drive the frontend with these names, so a rename on either side fails
// one suite instead of compiling clean on both.
import { readFileSync } from "node:fs";

export interface CoreNames {
  menuEvents: { openSettings: string; openAbout: string };
  itemKeys: Array<{ hash: string | null; pathId: number; key: string }>;
  backgroundWork: { classIds: string[]; classStates: string[]; reasons: string[] };
  transcriptStatus: { pending: string; ready: string; failed: string };
  transcribeEvents: { progress: string; done: string; error: string; cancelled: string };
}

export const CORE_NAMES = JSON.parse(
  readFileSync("tests/fixtures/core-frontend-names.json", "utf8"),
) as CoreNames;
