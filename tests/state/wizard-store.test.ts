// The wizard's Finish step.
//
// cacheDir is not an ordinary config key: the cache root is a live
// process-wide value that only `move_cache` (or first-run setup) commits.
// Patching config alone leaves derives writing to the new directory while
// every mediacache:// read still resolves against the old one, so the entire
// grid renders as placeholders for the rest of the session.

import { beforeEach, describe, expect, it } from "vitest";
import { useWizardStore } from "../../src/state/wizard-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

function patchConfigPayloads(): Array<Record<string, unknown>> {
  return invokeCalls
    .filter((c) => c.command === "patch_config")
    .map((c) => (c.args.patch ?? c.args) as Record<string, unknown>);
}

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    patch_config: () => ({}),
    patch_state: () => ({}),
    move_cache: () => null,
    start_scan: () => true,
    get_section_counts: () => [],
  });
  useWizardStore.setState({
    open: true,
    step: 3,
    dirs: [{ path: "/root", counting: false }] as never,
    timezone: "Asia/Tokyo",
    cacheDir: "/fast-ssd/onecopy-cache",
  });
});

describe("finish", () => {
  it("commits the cache directory through move_cache", async () => {
    await useWizardStore.getState().finish();

    const move = invokeCalls.find((c) => c.command === "move_cache");
    expect(move).toBeTruthy();
    expect(move?.args.newDir).toBe("/fast-ssd/onecopy-cache");
  });

  it("keeps cacheDir out of the config patch", async () => {
    await useWizardStore.getState().finish();

    for (const payload of patchConfigPayloads()) {
      expect(Object.keys(payload)).not.toContain("cacheDir");
    }
  });

  it("still saves the directories and timezone", async () => {
    await useWizardStore.getState().finish();

    const merged = Object.assign({}, ...patchConfigPayloads()) as Record<
      string,
      unknown
    >;
    expect(merged.sourceDirs).toEqual(["/root"]);
    expect(merged.defaultTimezone).toBe("Asia/Tokyo");
  });

  it("does not call move_cache when no directory was chosen", async () => {
    useWizardStore.setState({ cacheDir: null });

    await useWizardStore.getState().finish();

    expect(invokeCalls.some((c) => c.command === "move_cache")).toBe(false);
  });
});
