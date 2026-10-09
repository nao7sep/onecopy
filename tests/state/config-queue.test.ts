// One queue for every config write: writes reach the core in the order they
// were made, and settings published before they are saved stay published
// across unrelated saves.

import { beforeEach, expect, it } from "vitest";
import { resetConfigWritesForTests, useAppStore } from "../../src/state/app-store";
import { setPlaybackVolume } from "../../src/workflows/playback";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

// What config.json holds: the core merges each save into it and replies with
// the whole document.
let disk: Record<string, unknown> = {};
let release: Array<() => void> = [];

async function settle(): Promise<void> {
  for (let index = 0; index < 20; index += 1) await Promise.resolve();
}

function sent(): unknown[] {
  return invokeCalls.filter((call) => call.command === "save_config").map((call) => call.args.changes);
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  resetConfigWritesForTests();
  disk = { screenPriority: ["a", "b", "c"], soundEnabled: true, playbackVolume: 0.7, autoplay: true };
  release = [];
  mockCommands({
    save_config: ({ changes }) => new Promise((resolve) => {
      release.push(() => {
        disk = { ...disk, ...(changes as object) };
        resolve(disk);
      });
    }),
    log_event: () => null,
  });
  useAppStore.setState({
    appData: { config: { ...disk }, state: {}, dataRoot: "/app", debugEnabled: false, quarantines: [] },
  });
});

it("sends overlapping saves one at a time, so the file and the interface end on the last choice", async () => {
  const first = useAppStore.getState().saveConfig({ screenPriority: ["b", "a", "c"] });
  const second = useAppStore.getState().saveConfig({ screenPriority: ["b", "c", "a"] });
  await settle();
  expect(sent()).toHaveLength(1);

  release.shift()!();
  await first;
  await settle();
  expect(sent()).toHaveLength(2);
  release.shift()!();
  await second;

  expect(disk.screenPriority).toEqual(["b", "c", "a"]);
  expect(useAppStore.getState().appData?.config.screenPriority).toEqual(["b", "c", "a"]);
});

it("keeps a volume change waiting for its save through an unrelated save", async () => {
  setPlaybackVolume(0.3);
  const unrelated = useAppStore.getState().saveConfig({ autoplay: false });
  await settle();
  release.shift()!();
  await unrelated;

  // The reply is the file, which does not hold 0.3 yet.
  expect(disk.playbackVolume).toBe(0.7);
  expect(useAppStore.getState().appData?.config).toMatchObject({ autoplay: false, playbackVolume: 0.3 });
});

it("computes a change from the settings as saved when its turn comes", async () => {
  const first = useAppStore.getState().saveConfig({ screenPriority: ["c"] });
  const appended = useAppStore.getState().saveConfig((config) => ({
    screenPriority: [...(config.screenPriority as string[]), "d"],
  }));
  const skipped = useAppStore.getState().saveConfig(() => null);
  await settle();
  release.shift()!();
  await first;
  await settle();
  release.shift()!();
  await appended;

  expect(await skipped).toBeNull();
  expect(sent()).toEqual([{ screenPriority: ["c"] }, { screenPriority: ["c", "d"] }]);
  expect(disk.screenPriority).toEqual(["c", "d"]);
});
